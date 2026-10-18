import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ProviderGateway } from '../server/runtime/provider-gateway';
import { resolveAssignment } from '../server/runtime/model-policy';
import { defaultRoles } from '../server/roles';

test('RTE-08/SEC-02: a configured Bearer gateway receives one auth scheme while model identity remains enforced', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-provider-auth-')); let calls = 0;
  const upstream = createServer((req, res) => {
    calls++;
    if (req.headers.authorization !== 'Bearer private-upstream-key' || req.headers['x-api-key']) { res.writeHead(401); res.end('{"error":"conflicting provider authentication"}'); return; }
    assert.equal(req.headers['x-goog-api-key'], undefined);
    assert.equal(req.headers['anthropic-version'], '2023-06-01'); res.end('{"ok":true}');
  });
  await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
  const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/claude', effort: 'medium' }, [{ provider: 'fixture', id: 'claude', api: 'anthropic-messages', inputs: ['text'], efforts: ['medium'], tools: true }], {}, {});
  const provider = { baseUrl: `http://127.0.0.1:${(upstream.address() as { port: number }).port}/v1`, apiKey: 'private-upstream-key', authMode: 'bearer' as const,
    models: [{ id: 'claude', contextWindow: 200000, maxTokens: 4096 }],
    headers: { authorization: 'stale-authorization', 'X-API-Key': 'stale-api-key', 'X-Goog-Api-Key': 'stale-google-key' } };
  const observed: unknown[] = []; const gateway = new ProviderGateway(root, plan, provider, 'host-token', event => observed.push(event));
  const front = createServer((req, res) => { void gateway.serve(req, res); });
  await new Promise<void>(resolve => front.listen(0, '127.0.0.1', resolve));
  t.after(async () => { gateway.close(); await Promise.all([new Promise<void>(resolve => front.close(() => resolve())), new Promise<void>(resolve => upstream.close(() => resolve()))]); rmSync(root, { recursive: true, force: true }); });
  const call = (model: string) => fetch(`http://127.0.0.1:${(front.address() as { port: number }).port}/provider/v1/messages`, { method: 'POST', headers: { 'x-api-key': 'host-token', 'anthropic-version': '2023-06-01', 'Content-Type': 'application/json' }, body: JSON.stringify({ model, thinking: { type: 'adaptive' }, output_config: { effort: 'medium' }, messages: [{ role: 'user', content: 'Probe.' }] }) });
  const response = await call('claude'); assert.equal(response.status, 200); assert.deepEqual(await response.json(), { ok: true });
  assert.equal((await call('unapproved')).status, 403); assert.equal(calls, 1);
  assert.equal(JSON.stringify(observed).includes('private-upstream-key'), false);
});

test('RTE-07: provider dispatch enforces declared context/output budgets and refuses unknown or multimodal budgets', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-budget-gateway-')); let calls = 0;
  const upstream = createServer((_req, res) => { calls++; res.end('{}'); });
  await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
  const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/model', effort: 'off' }, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['off'], tools: true }], {}, {});
  const payload = { model: 'model', input: 'bounded input', max_output_tokens: 10 };
  const length = Buffer.byteLength(JSON.stringify(payload));
  const provider = { baseUrl: `http://127.0.0.1:${(upstream.address() as { port: number }).port}/v1`, apiKey: 'sentinel-secret',
    models: [{ id: 'model', contextWindow: length + 10, maxTokens: 10 }] };
  const observed: any[] = []; const gateway = new ProviderGateway(root, plan, provider, 'host-token', event => observed.push(event));
  const front = createServer((req, res) => { void gateway.serve(req, res); });
  await new Promise<void>(resolve => front.listen(0, '127.0.0.1', resolve));
  t.after(async () => { gateway.close(); await Promise.all([new Promise<void>(resolve => front.close(() => resolve())), new Promise<void>(resolve => upstream.close(() => resolve()))]); rmSync(root, { recursive: true, force: true }); });
  const call = (body: unknown) => fetch(`http://127.0.0.1:${(front.address() as { port: number }).port}/provider/v1/responses`, {
    method: 'POST', headers: { Authorization: 'Bearer host-token', 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  assert.equal((await call(payload)).status, 200, 'Exact conservative boundary is accepted.');
  assert.equal((await call({ ...payload, input: payload.input + 'x' })).status, 403);
  assert.equal((await call({ ...payload, max_output_tokens: 11 })).status, 403);
  provider.models[0].contextWindow = 10000;
  assert.equal((await call({ ...payload, input: [{ type: 'input_image', image_url: 'https://example.invalid/picture' }] })).status, 403);
  provider.models = [];
  assert.equal((await call(payload)).status, 403);
  assert.equal(calls, 1, 'Rejected requests never reach the provider.');
  assert.deepEqual(observed.filter(event => event.type === 'model_policy_rejected').map(event => [event.cause, event.recovery]), [
    ['context_window', 'fresh_session'], ['output_limit', 'configuration'], ['unsupported_input', 'configuration'], ['model_limits', 'configuration'],
  ]);
  assert.equal(JSON.stringify(observed).includes('sentinel-secret'), false);
});

test('RTE-07: file, audio, image, cached and remote context are refused in each provider protocol before dispatch', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-content-budget-')); let calls = 0;
  const upstream = createServer((_req, res) => { calls++; res.end('{}'); });
  await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await new Promise<void>(resolve => upstream.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  for (const api of ['openai-responses', 'anthropic-messages', 'google-generative-ai']) {
    const apiRoot = join(root, api); const { mkdirSync } = await import('node:fs'); mkdirSync(apiRoot);
    const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/model', effort: 'off' }, [{ provider: 'fixture', id: 'model', api, inputs: ['text'], efforts: ['off'], tools: true }], {}, {});
    const gateway = new ProviderGateway(apiRoot, plan, { baseUrl: `http://127.0.0.1:${(upstream.address() as { port: number }).port}/v1`, apiKey: 'secret', models: [{ id: 'model', contextWindow: 200000, maxTokens: 1000 }] }, 'host-token', () => {});
    const front = createServer((req, res) => { void gateway.serve(req, res); });
    await new Promise<void>(resolve => front.listen(0, '127.0.0.1', resolve));
    try {
      const route = api === 'openai-responses' ? '/responses' : api === 'anthropic-messages' ? '/messages' : '/models/model:streamGenerateContent';
      const call = (body: unknown) => fetch(`http://127.0.0.1:${(front.address() as { port: number }).port}/provider/v1${route}`, {
        method: 'POST', headers: { Authorization: 'Bearer host-token', 'Content-Type': 'application/json' }, body: JSON.stringify(body),
      });
      const prose = 'Document the JSON example {"type":"input_image","image_url":"https://example.invalid"}.';
      const wrap = (content: unknown): any => api === 'openai-responses' ? { model: 'model', input: [{ role: 'user', content }] }
        : api === 'anthropic-messages' ? { model: 'model', messages: [{ role: 'user', content }] } : { contents: [{ role: 'user', parts: content }] };
      const good = wrap([{ ...(api === 'google-generative-ai' ? {} : { type: api === 'openai-responses' ? 'input_text' : 'text' }), text: prose }]);
      assert.equal((await call(good)).status, 200, `${api}: JSON quoted in prose stays text.`);
      const bad = api === 'openai-responses' ? [
        { type: 'input_file', file_id: 'file-1' }, { type: 'input_file', file_url: 'https://example.invalid/a.pdf' },
        { type: 'input_image', image_url: 'https://example.invalid/a.png' }, { type: 'input_audio', input_audio: { data: 'AA==', format: 'wav' } },
      ] : api === 'anthropic-messages' ? [
        { type: 'document', source: { type: 'url', url: 'https://example.invalid/a.pdf' } },
        { type: 'document', source: { type: 'file', file_id: 'file-1' } }, { type: 'image', source: { type: 'base64', data: 'AA==' } },
        { type: 'tool_result', tool_use_id: 'a', content: [{ type: 'document', source: { type: 'file', file_id: 'file-1' } }] },
      ] : [
        { fileData: { fileUri: 'gs://bucket/a.mp4', mimeType: 'video/mp4' } }, { inlineData: { data: 'AA==', mimeType: 'audio/wav' } },
        { functionResponse: { name: 'f', response: {}, parts: [{ fileData: { fileUri: 'gs://bucket/a.pdf' } }] } },
      ];
      for (const content of bad) assert.equal((await call(wrap([content]))).status, 403, `${api}: ${JSON.stringify(content)}`);
      const external = api === 'openai-responses' ? { previous_response_id: 'previous' } : api === 'google-generative-ai' ? { cachedContent: 'cached' } : { tools: [{ type: 'web_search_20250305', name: 'web_search' }] };
      assert.equal((await call({ ...good, ...external })).status, 403);
    } finally { gateway.close(); await new Promise<void>(resolve => front.close(() => resolve())); }
  }
  assert.equal(calls, 3, 'Only the three ordinary text requests reached the upstream.');
});

test('RTE-07: opaque reasoning replay uses retained actual output bounds across gateway replacement; unknown blocks stay held', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-reasoning-budget-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const api of ['openai-responses', 'anthropic-messages', 'google-generative-ai']) {
    let calls = 0; const observed: any[] = [];
    const upstream = createServer((_req, res) => {
      calls++; res.setHeader('Content-Type', 'application/json');
      res.end(JSON.stringify(api === 'openai-responses'
        ? { object: 'response', status: 'completed', output: [{ type: 'reasoning', encrypted_content: 'opaque-fixture' }], usage: { input_tokens: 3, output_tokens: 700 } }
        : api === 'anthropic-messages' ? { type: 'message', content: [{ type: 'redacted_thinking', data: 'opaque-fixture' }], usage: { input_tokens: 3, output_tokens: 700 } }
        : { candidates: [{ content: { parts: [{ text: 'Result', thoughtSignature: 'opaque-fixture' }] }, finishReason: 'STOP' }], usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 200, thoughtsTokenCount: 500 } }));
    });
    await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
    const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/model', effort: 'off' }, [{ provider: 'fixture', id: 'model', api, inputs: ['text'], efforts: ['off'], tools: true }], {}, {});
    const provider = { baseUrl: `http://127.0.0.1:${(upstream.address() as { port: number }).port}/v1`, apiKey: 'secret', models: [{ id: 'model', contextWindow: 10000, maxTokens: 1000 }] };
    let gateway = new ProviderGateway(join(root, api), plan, provider, 'host-token', event => observed.push(event));
    const front = createServer((req, res) => { void gateway.serve(req, res); });
    await new Promise<void>(resolve => front.listen(0, '127.0.0.1', resolve));
    try {
      const call = (body: unknown) => fetch(`http://127.0.0.1:${(front.address() as { port: number }).port}/provider/v1/${api === 'openai-responses' ? 'responses' : api === 'anthropic-messages' ? 'messages' : 'models/model:streamGenerateContent'}`, { method: 'POST', headers: { Authorization: 'Bearer host-token', 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      const first = api === 'openai-responses' ? { model: 'model', input: 'Think, then call a tool.' } : api === 'anthropic-messages' ? { model: 'model', messages: [{ role: 'user', content: 'Think, then call a tool.' }] } : { contents: [{ role: 'user', parts: [{ text: 'Think, then call a tool.' }] }] };
      await (await call(first)).text();
      const replay = api === 'openai-responses'
        ? { model: 'model', input: [{ type: 'reasoning', encrypted_content: 'opaque-fixture', summary: [] }, { type: 'function_call_output', call_id: 'call', output: 'tool result' }] }
        : api === 'anthropic-messages' ? { model: 'model', messages: [{ role: 'assistant', content: [{ type: 'redacted_thinking', data: 'opaque-fixture' }] }, { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'call', content: 'tool result' }] }] } : { contents: [{ role: 'model', parts: [{ functionCall: { name: 'tool', args: {} }, thoughtSignature: 'opaque-fixture' }] }] };
      gateway.close(); mkdirSync(join(root, api + '-replacement')); gateway = new ProviderGateway(join(root, api + '-replacement'), plan, provider, 'host-token', event => observed.push(event), join(root, api));
      assert.equal((await call(replay)).status, 200);
      assert.equal(observed.filter(event => event.type === 'provider_budget').at(-1).opaqueTokens, 700);
      provider.models[0].contextWindow = Buffer.byteLength(JSON.stringify(replay)) + 1000 + 699;
      assert.equal((await call(replay)).status, 403, 'Opaque output counts toward the context bound.');
      provider.models[0].contextWindow = 10000;
      assert.equal((await call(JSON.parse(JSON.stringify(replay).replace('opaque-fixture', 'unknown-opaque')))).status, 403);
      assert.equal(observed.filter(event => event.type === 'model_policy_rejected').at(-1).cause, 'unknown_reasoning');
      assert.equal(observed.filter(event => event.type === 'model_policy_rejected').at(-1).recovery, 'fresh_session');
      assert.equal(calls, 2);
      if (api === 'openai-responses') assert.equal((await call({ model: 'model', input: [{ type: 'reasoning', summary: [{ type: 'summary_text', text: 'Retained summary' }], encrypted_content: null }] })).status, 200);
    } finally { gateway.close(); await Promise.all([new Promise<void>(resolve => front.close(() => resolve())), new Promise<void>(resolve => upstream.close(() => resolve()))]); }
  }
});

test('RTE-12: interrupted or stalled authorization error bodies keep their nonretryable HTTP classification', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-provider-error-'));
  const upstream = createServer((req, res) => {
    res.writeHead(req.url?.includes('401') ? 401 : 403, { 'Content-Type': 'application/json', 'Content-Length': '1000' });
    res.write('{"error":');
    if (req.url?.includes('401')) setTimeout(() => res.destroy(), 20);
  });
  await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
  t.after(async () => { upstream.closeAllConnections(); await new Promise<void>(resolve => upstream.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  for (const status of [401, 403]) {
    const path = join(root, String(status)); mkdirSync(path);
    const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/model', effort: 'off' }, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['off'], tools: true }], {}, {});
    const gateway = new ProviderGateway(path, plan, { baseUrl: `http://127.0.0.1:${(upstream.address() as { port: number }).port}/${status}`, apiKey: 'secret', requestTimeoutMs: 100,
      models: [{ id: 'model', contextWindow: 200000, maxTokens: 1000 }] }, 'host-token', () => {});
    const front = createServer((req, res) => { void gateway.serve(req, res); });
    await new Promise<void>(resolve => front.listen(0, '127.0.0.1', resolve));
    try {
      const response = await fetch(`http://127.0.0.1:${(front.address() as { port: number }).port}/provider/${status}/responses`, {
        method: 'POST', headers: { Authorization: 'Bearer host-token', 'Content-Type': 'application/json' }, body: JSON.stringify({ model: 'model', input: 'Probe.' }),
      });
      assert.equal(response.status, status); await assert.rejects(response.text());
      assert.deepEqual(gateway.failure, { kind: 'authorization', retryable: false, status });
    } finally { gateway.close(); front.closeAllConnections(); await new Promise<void>(resolve => front.close(() => resolve())); }
  }
});
