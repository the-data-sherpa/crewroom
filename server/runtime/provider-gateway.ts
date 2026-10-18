import type { IncomingMessage, ServerResponse } from 'node:http';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { AssignmentModel } from './model-policy';
import { UsageReader } from './provider-usage';
import { hasBoundedTextInput, ReasoningBudget } from './provider-input';

interface Provider { requestTimeoutMs?: number; baseUrl: string; apiKey: string; headers?: Record<string, string>; authMode?: 'native' | 'bearer';
  models?: { id: string; contextWindow?: number; maxTokens?: number }[] }

/** Keep retries inside the admitted provider/model and the verified session's serialized effort. */
export class ProviderGateway {
  failure?: { kind: string; retryable: boolean; status?: number };
  private fail(failure: { kind: string; retryable: boolean; status?: number }) {
    this.failure = failure; this.observe({ type: 'provider_failure', ...failure });
  }
  private active = new Set<AbortController>();
  private stopped = false;
  private reasoning: ReasoningBudget;
  constructor(private root: string, private plan: AssignmentModel, private provider: Provider, private token: string,
    private observe: (event: any) => void, budgetRoot = root) {
    this.reasoning = new ReasoningBudget(budgetRoot, plan.resolved.selector);
    if (provider.requestTimeoutMs !== undefined && (!Number.isSafeInteger(provider.requestTimeoutMs) || provider.requestTimeoutMs < 100 || provider.requestTimeoutMs > 3600000)) throw new Error('Provider request timeout must be 100–3600000 ms.');
    if (provider.authMode !== undefined && !['native', 'bearer'].includes(provider.authMode)) throw new Error('Unsupported provider authentication mode.');
  }

  async serve(req: IncomingMessage, res: ServerResponse) {
    const url = new URL(req.url!, 'http://127.0.0.1');
    const authenticated = req.headers.authorization === `Bearer ${this.token}` || req.headers['x-api-key'] === this.token
      || req.headers['x-goog-api-key'] === this.token || url.searchParams.get('key') === this.token;
    if (!authenticated || req.method !== 'POST' || this.stopped) { res.writeHead(403); res.end('{}'); return; }
    let raw = ''; for await (const chunk of req) { raw += chunk; if (raw.length > 4 * 1024 * 1024) throw new Error('Provider request exceeded its limit.'); }
    const body = JSON.parse(raw);
    const model = this.plan.resolved;
    const base = new URL(this.provider.baseUrl);
    const basePath = base.pathname.replace(/\/$/, '');
    if (!url.pathname.startsWith(`/provider${basePath}/`)) { res.writeHead(403); res.end('{}'); return; }
    const route = url.pathname.slice(`/provider${basePath}`.length);
    let wire: unknown;
    if (model.api === 'openai-responses' && route === '/responses') {
      wire = { model: body.model, effort: body.reasoning?.effort ?? null };
    } else if (model.api === 'anthropic-messages' && ['/messages', '/v1/messages'].includes(route)) {
      wire = { model: body.model, thinking: body.thinking?.type ?? null, budget: body.thinking?.budget_tokens ?? null, effort: body.output_config?.effort ?? null };
    } else if (model.api === 'google-generative-ai' && route.startsWith('/models/') && route.endsWith(':streamGenerateContent')) {
      const thinking = body.generationConfig?.thinkingConfig;
      wire = { model: decodeURIComponent(route.slice('/models/'.length, -':streamGenerateContent'.length)),
        effort: thinking?.thinkingLevel ?? null, budget: thinking?.thinkingBudget ?? null, includeThoughts: thinking?.includeThoughts ?? null };
    } else { res.writeHead(403); res.end('{}'); return; }
    const identity = wire as { model: string; effort?: string | null };
    const baseline = join(this.root, 'provider-policy.json');
    const serialized = JSON.stringify({ selector: model.selector, effort: model.effort, wire });
    const invalidOpenAiEffort = model.api === 'openai-responses' && (model.effort === 'off'
      ? identity.effort !== null && identity.effort !== 'none' : identity.effort !== model.effort);
    if (identity.model !== model.id || invalidOpenAiEffort || existsSync(baseline) && readFileSync(baseline, 'utf8') !== serialized) {
      this.observe({ type: 'model_policy_rejected', requestedModel: model.selector, requestedEffort: model.effort, attempted: wire });
      res.writeHead(403, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: { type: 'crewroom_assignment_policy', message: 'Crewroom rejected a model or effort substitution.' } })); return;
    }
    const limits = this.provider.models?.filter(item => item.id === model.id);
    const configured = limits?.length === 1 ? limits[0] : undefined;
    const output = body.max_output_tokens ?? body.max_tokens ?? body.generationConfig?.maxOutputTokens ?? configured?.maxTokens;
    // A conservative text budget, not a tokenizer or a usage claim. Counting all serialized UTF-8
    // bytes overestimates text tokens and framing; unsupported multimodal requests remain refused.
    const inputBytes = Buffer.byteLength(raw, 'utf8');
    const validLimit = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) > 0;
    let opaqueTokens = 0; let unknownReasoning = false;
    const bounded = hasBoundedTextInput(model.api, body, (kind, value) => {
      const tokens = this.reasoning.tokens(kind, value);
      if (tokens === undefined) { unknownReasoning = true; return false; }
      opaqueTokens += tokens; return true;
    });
    const cause = !validLimit(configured?.contextWindow) || !validLimit(configured?.maxTokens) ? 'model_limits'
      : !validLimit(output) || output > configured.maxTokens ? 'output_limit'
      : !bounded ? unknownReasoning ? 'unknown_reasoning' : 'unsupported_input'
      : inputBytes + opaqueTokens + output > configured.contextWindow ? 'context_window' : undefined;
    if (cause) {
      this.observe({ type: 'model_policy_rejected', code: 'input_budget', cause,
        recovery: cause === 'context_window' || cause === 'unknown_reasoning' ? 'fresh_session' : 'configuration',
        reason: 'Provider input budget is unavailable or exceeded; multimodal input has no verified budget.',
        budget: { inputBytes, opaqueTokens, outputTokens: output ?? null, contextWindow: configured?.contextWindow ?? null, maxOutputTokens: configured?.maxTokens ?? null, method: 'serialized-utf8-bytes-plus-reported-opaque-output' } });
      res.writeHead(403, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: { type: 'crewroom_assignment_policy', message: 'Input budget is unavailable or exceeded. Retain the work and configure verified model limits or shorten the input.' } })); return;
    }
    this.observe({ type: 'provider_budget', inputBytes, opaqueTokens, outputTokens: output, contextWindow: configured!.contextWindow, method: 'serialized-utf8-bytes-plus-reported-opaque-output' });
    if (!existsSync(baseline)) writeFileSync(baseline, serialized, { mode: 0o600, flag: 'wx', flush: true });
    const upstream = new URL(base.origin + url.pathname.slice('/provider'.length));
    for (const key of ['beta', 'alt']) if (url.searchParams.has(key)) upstream.searchParams.set(key, url.searchParams.get(key)!);
    const headers: Record<string, string> = { 'Content-Type': 'application/json', ...Object.fromEntries(Object.entries(this.provider.headers ?? {}).filter(([key]) => !/^(authorization|x-api-key|x-goog-api-key)$/i.test(key))) };
    if (model.api === 'anthropic-messages') {
      if (this.provider.authMode === 'bearer') headers.Authorization = `Bearer ${this.provider.apiKey}`;
      else headers['x-api-key'] = this.provider.apiKey;
      for (const key of ['anthropic-version', 'anthropic-beta']) if (typeof req.headers[key] === 'string') headers[key] = req.headers[key];
    } else if (model.api === 'google-generative-ai' && this.provider.authMode !== 'bearer') headers['x-goog-api-key'] = this.provider.apiKey;
    else headers.Authorization = `Bearer ${this.provider.apiKey}`;
    const controller = new AbortController(); this.active.add(controller);
    let timedOut = false;
    const deadline = setTimeout(() => { timedOut = true; controller.abort(); }, this.provider.requestTimeoutMs ?? 300000);
    res.once('close', () => controller.abort());
    const opaqueBlocks: { kind: string; value: string }[] = [];
    let response: Response | undefined; let usage: UsageReader | undefined; let interrupted = false;
    try {
      this.failure = undefined;
      this.observe({ type: 'provider_request', identity: wire });
      response = await fetch(upstream, { method: 'POST', redirect: 'error', headers, body: raw, signal: controller.signal });
      if (!response.ok) this.fail({ kind: response.status === 401 || response.status === 403 ? 'authorization'
        : response.status === 429 ? 'rate_limit' : response.status === 408 || response.status === 504 ? 'timeout' : response.status >= 500 ? 'unavailable' : 'rejected',
        status: response.status, retryable: [408, 429].includes(response.status) || response.status >= 500 });
      res.writeHead(response.status, { 'Content-Type': response.headers.get('content-type') ?? 'application/json', 'Cache-Control': 'no-store' });
      usage = new UsageReader(model.api, (response.headers.get('content-type') ?? '').includes('text/event-stream'), (kind, value) => opaqueBlocks.push({ kind, value }));
      if (response.body) for await (const chunk of response.body) { usage.push(chunk); res.write(chunk); }
      res.end();
    } catch {
      interrupted = true;
      if (!this.stopped && (!response || response.ok)) this.fail({ kind: timedOut ? 'timeout' : 'disconnect', retryable: true });
      if (!res.headersSent) { res.writeHead(502); res.end('{"error":{"message":"Provider response was interrupted."}}'); }
      else res.destroy();
    } finally {
      clearTimeout(deadline); this.active.delete(controller);
      // One receipt per upstream request: actual counts, or an explicit reason they are unavailable.
      const receipt = usage ? usage.finish(response!.status, interrupted)
        : { type: 'provider_usage', status: 0, state: 'unavailable', reason: 'The provider request failed before a response.' } as const;
      if (receipt.state === 'actual') this.reasoning.remember(opaqueBlocks, receipt.usage.output);
      this.observe(receipt);
      const outcome = usage?.outcome();
      if (response?.ok && !interrupted && outcome) this.fail({ kind: outcome, retryable: true });
    }
  }
  close() { this.stopped = true; for (const controller of this.active) controller.abort(); }
}
