import assert from 'node:assert/strict';
import { test } from 'node:test';
import { UsageReader } from '../server/runtime/provider-usage';

const sse = (...events: object[]) => new TextEncoder().encode(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join(''));
const read = (api: string, stream: boolean, chunks: Uint8Array[], status = 200, interrupted = false) => {
  const reader = new UsageReader(api, stream);
  for (const chunk of chunks) reader.push(chunk);
  return reader.finish(status, interrupted);
};

test('R5: provider usage is read from each API family completion report, including split stream chunks', () => {
  const openai = sse({ type: 'response.created', response: { usage: null } },
    { type: 'response.completed', response: { usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120, input_tokens_details: { cached_tokens: 60 }, output_tokens_details: { reasoning_tokens: 8 } } } });
  // A chunk boundary inside a data line must not lose the report.
  assert.deepEqual(read('openai-responses', true, [openai.slice(0, 90), openai.slice(90)]),
    { type: 'provider_usage', status: 200, state: 'actual', usage: { input: 100, output: 20, cacheRead: 60, cacheWrite: 0, reasoning: 8, total: 120 } });
  const anthropic = sse({ type: 'message_start', message: { usage: { input_tokens: 5, cache_read_input_tokens: 40, cache_creation_input_tokens: 10, output_tokens: 1 } } },
    { type: 'content_block_delta', delta: { type: 'text_delta', text: 'hi' } }, { type: 'message_delta', usage: { output_tokens: 30 } }, { type: 'message_stop' });
  assert.deepEqual(read('anthropic-messages', true, [anthropic]),
    { type: 'provider_usage', status: 200, state: 'actual', usage: { input: 55, output: 30, cacheRead: 40, cacheWrite: 10, reasoning: 0, total: 85 } });
  const incomplete = sse({ type: 'response.incomplete', response: { status: 'incomplete', incomplete_details: { reason: 'max_output_tokens' }, usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } } });
  assert.equal(read('openai-responses', true, [incomplete]).state, 'actual', 'A token-limited response still consumed and reported tokens.');
  const google = new TextEncoder().encode(JSON.stringify([
    { candidates: [{ content: { parts: [{ text: 'a' }] } }], usageMetadata: { promptTokenCount: 9, candidatesTokenCount: 1 } },
    { candidates: [{ content: { parts: [{ text: 'b' }] }, finishReason: 'STOP' }], usageMetadata: { promptTokenCount: 9, candidatesTokenCount: 4, thoughtsTokenCount: 3, cachedContentTokenCount: 2, totalTokenCount: 16 } }]));
  assert.deepEqual(read('google-generative-ai', false, [google]),
    { type: 'provider_usage', status: 200, state: 'actual', usage: { input: 9, output: 7, cacheRead: 2, cacheWrite: 0, reasoning: 3, total: 16 } });
});

test('R5: incomplete, failed and unreported responses are unavailable rather than zero', () => {
  const started = sse({ type: 'message_start', message: { usage: { input_tokens: 5, output_tokens: 0 } } });
  assert.equal(read('anthropic-messages', true, [started]).state, 'unavailable', 'Prompt counts alone are not a completed response.');
  const completed = sse({ type: 'response.completed', response: { usage: { input_tokens: 1, output_tokens: 1 } } });
  assert.match(JSON.stringify(read('openai-responses', true, [completed], 200, true)), /interrupted/);
  assert.match(JSON.stringify(read('openai-responses', false, [new TextEncoder().encode('{"error":{}}')], 429)), /HTTP 429/);
  assert.match(JSON.stringify(read('openai-responses', true, [sse({ type: 'response.completed', response: {} })])), /did not include a usage report/);
});


test('RTE-12: protocol completion and usage do not turn malformed or empty output into a usable result', () => {
  const fixtures = [
    { api: 'openai-responses', empty: { type: 'response.completed', response: { output: [], usage: { input_tokens: 1, output_tokens: 0 } } },
      valid: { type: 'response.completed', response: { output: [{ type: 'function_call', name: 'send_message', arguments: '{}' }] } } },
    { api: 'anthropic-messages', empty: { type: 'message_stop' }, valid: { type: 'content_block_start', content_block: { type: 'tool_use', name: 'send_message' } } },
    { api: 'google-generative-ai', empty: { candidates: [{ content: { parts: [] }, finishReason: 'STOP' }] },
      valid: { candidates: [{ content: { parts: [{ text: 'Result' }] }, finishReason: 'STOP' }] } },
  ];
  for (const fixture of fixtures) {
    const empty = new UsageReader(fixture.api, true); empty.push(sse(fixture.empty)); empty.finish(200, false);
    assert.equal(empty.outcome(), 'empty_final', fixture.api);
    const malformed = new UsageReader(fixture.api, true); malformed.push(new TextEncoder().encode('data: {bad-json}\n\n')); malformed.finish(200, false);
    assert.equal(malformed.outcome(), 'malformed', fixture.api);
    const valid = new UsageReader(fixture.api, true); valid.push(sse(fixture.valid, fixture.empty)); valid.finish(200, false);
    assert.equal(valid.outcome(), undefined, fixture.api);
  }
});


test('RTE-12: a completed refusal is a response; private reasoning alone is not a final answer', () => {
  const refusal = new UsageReader('openai-responses', true);
  refusal.push(sse({ type: 'response.completed', response: { output: [{ type: 'message', content: [{ type: 'refusal', refusal: 'I cannot help with that.' }] }] } }));
  refusal.finish(200, false); assert.equal(refusal.outcome(), undefined);
  const delta = new UsageReader('openai-responses', true);
  delta.push(sse({ type: 'response.refusal.delta', delta: 'I cannot help.' }, { type: 'response.completed', response: { output: [] } }));
  delta.finish(200, false); assert.equal(delta.outcome(), undefined);
  for (const [api, event] of [
    ['openai-responses', { type: 'response.completed', response: { output: [{ type: 'reasoning', content: [{ type: 'reasoning_text', text: 'Internal thought' }] }] } }],
    ['google-generative-ai', { candidates: [{ content: { parts: [{ thought: true, text: 'Internal thought' }] }, finishReason: 'STOP' }] }],
  ] as const) {
    const reader = new UsageReader(api, true); reader.push(sse(event)); reader.finish(200, false);
    assert.equal(reader.outcome(), 'empty_final', api);
  }
});
