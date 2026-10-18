import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/** Opaque reasoning is budgeted only from a completed provider response's reported output.
 * Hashes and counts survive attempt replacement; opaque contents are never copied to this ledger.
 */
export class ReasoningBudget {
  private values: Record<string, number>;
  private path: string;
  constructor(root: string, private selector: string) {
    mkdirSync(root, { recursive: true, mode: 0o700 }); this.path = join(root, 'reasoning-budget.json');
    this.values = existsSync(this.path) ? JSON.parse(readFileSync(this.path, 'utf8')) : {};
    if (!this.values || Array.isArray(this.values) || typeof this.values !== 'object' || Object.entries(this.values).some(([key, value]) => !/^[a-f0-9]{64}$/.test(key) || !Number.isSafeInteger(value) || value < 0)) throw new Error('Saved reasoning budget is invalid; retain the session for inspection.');
  }
  private key(kind: string, value: string) { return createHash('sha256').update(JSON.stringify([this.selector, kind, value])).digest('hex'); }
  tokens(kind: string, value: string) { return this.values[this.key(kind, value)]; }
  remember(blocks: { kind: string; value: string }[], output: number) {
    if (!blocks.length) return;
    // Assign the whole response output to each block. Multiple blocks overcount safely.
    for (const block of blocks) {
      const key = this.key(block.kind, block.value); this.values[key] = Math.max(output, this.values[key] ?? 0);
    }
    const temporary = this.path + '.' + randomUUID();
    writeFileSync(temporary, JSON.stringify(this.values), { mode: 0o600, flush: true }); renameSync(temporary, this.path);
  }
}

/** Only serialized text and local function exchanges have a byte-bounded input.
 * Inspect protocol content positions, never JSON-looking prose or tool arguments.
 */
export function hasBoundedTextInput(api: string, body: Record<string, any>, opaque: (kind: string, value: string) => boolean = () => false): boolean {
  const object = (value: unknown): value is Record<string, any> => !!value && typeof value === 'object' && !Array.isArray(value);
  const list = (value: unknown, check: (item: any) => boolean): boolean => Array.isArray(value) && value.every(item => object(item) && check(item));
  const text = (value: unknown, check: (item: any) => boolean): boolean => typeof value === 'string' || list(value, check);
  if (api === 'openai-responses') {
    const block = (item: any) => ['input_text', 'output_text', 'summary_text', 'reasoning_text'].includes(item.type) && typeof item.text === 'string'
      || item.type === 'refusal' && typeof item.refusal === 'string';
    return !body.previous_response_id && !body.conversation && !body.prompt
      && (body.tools === undefined || list(body.tools, item => item.type === 'function'))
      && text(body.input, item => {
        if (item.type === undefined || item.type === 'message') return text(item.content, block);
        if (item.type === 'function_call') return typeof item.arguments === 'string';
        if (item.type === 'function_call_output') return text(item.output, block);
        if (item.type === 'reasoning') return (item.encrypted_content == null || typeof item.encrypted_content === 'string' && opaque('openai-reasoning', item.encrypted_content)) && (item.summary === undefined || list(item.summary, block))
          && (item.content === undefined || list(item.content, block));
        return false;
      });
  }
  if (api === 'anthropic-messages') {
    const block = (item: any): boolean => {
      if (item.type === 'text') return typeof item.text === 'string';
      if (item.type === 'redacted_thinking') return typeof item.data === 'string' && opaque('anthropic-thinking', item.data);
      if (item.type === 'thinking') return typeof item.thinking === 'string';
      if (item.type === 'tool_use') return object(item.input);
      if (item.type === 'tool_result') return text(item.content, block);
      return false;
    };
    return (body.system === undefined || text(body.system, item => item.type === 'text' && typeof item.text === 'string'))
      && (body.tools === undefined || list(body.tools, item => item.type === undefined || item.type === 'custom'))
      && list(body.messages, item => text(item.content, block));
  }
  if (api === 'google-generative-ai') {
    const part = (item: any): boolean => {
      if (Object.keys(item).some(key => !['text', 'thought', 'thoughtSignature', 'functionCall', 'functionResponse'].includes(key))) return false;
      if (item.thoughtSignature !== undefined && (typeof item.thoughtSignature !== 'string' || item.thoughtSignature !== 'skip_thought_signature_validator' && !opaque('google-thinking', item.thoughtSignature))) return false;
      if (item.functionResponse !== undefined) return object(item.functionResponse) && object(item.functionResponse.response)
        && (item.functionResponse.parts === undefined || list(item.functionResponse.parts, part));
      return typeof item.text === 'string' || object(item.functionCall);
    };
    return !body.cachedContent
      && (body.tools === undefined || list(body.tools, item => Object.keys(item).every(key => key === 'functionDeclarations')))
      && (body.systemInstruction === undefined || object(body.systemInstruction) && list(body.systemInstruction.parts, part))
      && list(body.contents, item => list(item.parts, part));
  }
  return false;
}
