/** Token counts reported by the provider for one response. `input` includes cached prompt tokens. */
export interface TokenUsage { input: number; output: number; cacheRead: number; cacheWrite: number; reasoning: number; total: number }

/** The gateway's receipt for one upstream model response; usage is never inferred. */
export type ProviderUsage =
  | { type: 'provider_usage'; status: number; state: 'actual'; usage: TokenUsage }
  | { type: 'provider_usage'; status: number; state: 'unavailable'; reason: string };

const count = (value: unknown) => Number.isSafeInteger(value) && Number(value) >= 0 ? Number(value) : undefined;
const record = (value: unknown): Record<string, unknown> => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};

/**
 * Reads the provider's own usage report from a proxied response without altering it. A
 * response counts as actual only after the API's completion marker carries usage.
 */
export class UsageReader {
  private decoder = new TextDecoder();
  private pending = '';
  private buffered = 0;
  private overflow = false;
  private usage?: TokenUsage;
  private anthropic: Record<string, unknown> = {};
  private anthropicDone = false;
  private terminal = false;
  private meaningful = false;
  private malformed = false;

  constructor(private api: string, private stream: boolean, private opaque?: (kind: string, value: string) => void) {}

  push(chunk: Uint8Array) {
    if (this.overflow) return;
    this.pending += this.decoder.decode(chunk, { stream: true });
    if (!this.stream) {
      this.buffered += chunk.length;
      // A whole JSON body larger than this is not parsed; its usage is reported unavailable.
      if (this.buffered > 8 * 1024 * 1024) { this.overflow = true; this.pending = ''; }
      return;
    }
    let end;
    while ((end = this.pending.indexOf('\n')) >= 0) {
      const line = this.pending.slice(0, end).trim(); this.pending = this.pending.slice(end + 1);
      if (line.startsWith('data:')) this.data(line.slice(5).trim());
    }
    if (this.pending.length > 1024 * 1024) this.pending = '';
  }

  finish(status: number, interrupted: boolean): ProviderUsage {
    if (!this.stream && !this.overflow) this.data(this.pending + this.decoder.decode());
    const unavailable = (reason: string): ProviderUsage => ({ type: 'provider_usage', status, state: 'unavailable', reason });
    if (interrupted) return unavailable('The provider response was interrupted before it completed.');
    if (status < 200 || status >= 300) return unavailable(`The provider returned HTTP ${status} without a usage report.`);
    return this.usage ? { type: 'provider_usage', status, state: 'actual', usage: this.usage } : unavailable('The provider response did not include a usage report.');
  }

  /** Protocol completion does not establish useful content, repository evidence, or task completion. */
  outcome(): 'malformed' | 'empty_final' | undefined {
    return this.malformed || !this.terminal ? 'malformed' : !this.meaningful ? 'empty_final' : undefined;
  }

  private data(text: string) {
    if (!text || text === '[DONE]') return;
    let value: unknown;
    try { value = JSON.parse(text); } catch { this.malformed = true; return; }
    for (const item of Array.isArray(value) ? value : [value]) this.accept(record(item));
  }

  private accept(event: Record<string, unknown>) {
    if (this.api === 'openai-responses') {
      const response = record(event.response);
      if (event.type === 'response.completed' || !this.stream && event.object === 'response' && event.status === 'completed') this.terminal = true;
      const content = (value: unknown): boolean => { const item = record(value); return item.type === 'function_call' && typeof item.name === 'string'
        || item.type === 'output_text' && typeof item.text === 'string' && !!item.text.trim()
        || item.type === 'refusal' && typeof item.refusal === 'string' && !!item.refusal.trim()
        || item.type === 'message' && Array.isArray(item.content) && item.content.some(content); };
      if ((Array.isArray(response.output) && response.output.some(content)) || (Array.isArray(event.output) && event.output.some(content))
        || content(event.item) || ['response.output_text.delta', 'response.refusal.delta'].includes(String(event.type)) && typeof event.delta === 'string' && event.delta.trim()) this.meaningful = true;
      const items = [record(event.item), ...(Array.isArray(response.output) ? response.output : []), ...(Array.isArray(event.output) ? event.output : [])];
      for (const value of items) { const item = record(value); if (item.type === 'reasoning' && typeof item.encrypted_content === 'string') this.opaque?.('openai-reasoning', item.encrypted_content); }
      // Terminal incomplete/failed responses (for example max_output_tokens) still consumed and report tokens.
      const terminal = ['response.completed', 'response.incomplete', 'response.failed'].includes(String(event.type));
      const usage = record(terminal ? record(event.response).usage : !this.stream && event.object === 'response' && ['completed', 'incomplete', 'failed'].includes(String(event.status)) ? event.usage : undefined);
      const input = count(usage.input_tokens), output = count(usage.output_tokens);
      if (input === undefined || output === undefined) return;
      const reasoning = count(record(usage.output_tokens_details).reasoning_tokens) ?? 0;
      this.usage = { input, output, cacheRead: count(record(usage.input_tokens_details).cached_tokens) ?? 0, cacheWrite: 0, reasoning, total: count(usage.total_tokens) ?? input + output };
    } else if (this.api === 'anthropic-messages') {
      if (event.type === 'message_stop' || !this.stream && event.type === 'message' && event.stop_reason) this.terminal = true;
      const content = (value: unknown) => { const item = record(value); return item.type === 'tool_use' && typeof item.name === 'string' || typeof item.text === 'string' && !!item.text.trim(); };
      if (content(event.content_block) || content(event.delta) || Array.isArray(event.content) && event.content.some(content)) this.meaningful = true;
      const items = [record(event.content_block), ...(Array.isArray(event.content) ? event.content : [])];
      for (const value of items) { const item = record(value); if (item.type === 'redacted_thinking' && typeof item.data === 'string') this.opaque?.('anthropic-thinking', item.data); }
      // message_start carries the prompt counts; message_delta carries cumulative output and may restate input.
      const usage = event.type === 'message_start' ? record(record(event.message).usage) : event.type === 'message_delta' || (!this.stream && event.type === 'message') ? record(event.usage) : undefined;
      if (!usage) return;
      for (const [key, value] of Object.entries(usage)) if (count(value) !== undefined) this.anthropic[key] = value;
      if (event.type !== 'message_start') this.anthropicDone = true;
      const uncached = count(this.anthropic.input_tokens), output = count(this.anthropic.output_tokens);
      if (!this.anthropicDone || uncached === undefined || output === undefined) return;
      const cacheRead = count(this.anthropic.cache_read_input_tokens) ?? 0, cacheWrite = count(this.anthropic.cache_creation_input_tokens) ?? 0;
      const input = uncached + cacheRead + cacheWrite;
      this.usage = { input, output, cacheRead, cacheWrite, reasoning: 0, total: input + output };
    } else if (this.api === 'google-generative-ai') {
      for (const candidate of Array.isArray(event.candidates) ? event.candidates : []) {
        const parts = record(record(candidate).content).parts;
        if (typeof record(candidate).finishReason === 'string') this.terminal = true;
        if (Array.isArray(parts) && parts.some(value => { const part = record(value); return part.thought !== true && typeof part.text === 'string' && part.text.trim() || part.functionCall; })) this.meaningful = true;
        for (const value of Array.isArray(parts) ? parts : []) { const part = record(value); if (typeof part.thoughtSignature === 'string') this.opaque?.('google-thinking', part.thoughtSignature); }
      }
      const usage = record(event.usageMetadata);
      const finished = Array.isArray(event.candidates) && event.candidates.some(candidate => typeof record(candidate).finishReason === 'string');
      const input = count(usage.promptTokenCount), candidates = count(usage.candidatesTokenCount);
      if (!finished || input === undefined || candidates === undefined) return;
      const reasoning = count(usage.thoughtsTokenCount) ?? 0;
      this.usage = { input, output: candidates + reasoning, cacheRead: count(usage.cachedContentTokenCount) ?? 0, cacheWrite: 0, reasoning, total: count(usage.totalTokenCount) ?? input + candidates + reasoning };
    }
  }
}
