import { attemptModel } from './types';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { Workbench, Conflict } from '../workbench';

class MailHttpError extends Error {
  constructor(readonly status: number, readonly retryAfter: number) { super(`Rust Mail request failed (${status}).`); }
}
const delay = (milliseconds: number) => new Promise<void>(resolve => setTimeout(resolve, milliseconds));

export class RustMailClient {
  private session?: string;
  private initialized?: Promise<void>;
  constructor(private url: string, private token: string, private pause = delay, private requestBudgetMs = 12000) {
    const endpoint = new URL(url);
    if (endpoint.protocol !== 'http:' || endpoint.hostname !== '127.0.0.1' || endpoint.username || endpoint.password || endpoint.search || endpoint.hash) throw new Error('Rust Mail requires a loopback HTTP endpoint.');
  }

  private async rpc(method: string, params: unknown, notification = false, deadline = Date.now() + this.requestBudgetMs): Promise<any> {
    if (Date.now() >= deadline) throw new DOMException('Rust Mail request budget exhausted.', 'TimeoutError');
    const id = randomUUID();
    const response = await fetch(this.url, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(Math.max(1, deadline - Date.now())),
      headers: { 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream', Authorization: `Bearer ${this.token}`,
        ...(this.session ? { 'Mcp-Session-Id': this.session } : {}) },
      body: JSON.stringify({ jsonrpc: '2.0', method, params, ...(notification ? {} : { id }) }) });
    if (!response.ok) {
      const retry = response.headers.get('retry-after');
      const seconds = retry === null ? 0 : Number(retry);
      const retryAfter = Number.isFinite(seconds) ? Math.max(0, seconds * 1000) : Math.max(0, Date.parse(retry!) - Date.now());
      await response.body?.cancel();
      throw new MailHttpError(response.status, Number.isFinite(retryAfter) ? retryAfter : 0);
    }
    this.session = response.headers.get('Mcp-Session-Id') ?? this.session;
    const raw = await response.text();
    if (notification || !raw) return;
    const result = response.headers.get('content-type')?.includes('text/event-stream')
      ? raw.split('\n').filter(line => line.startsWith('data:')).map(line => JSON.parse(line.slice(5))).find(item => item.id === id)
      : JSON.parse(raw);
    if (!result || result.id !== id || result.error) throw new Error('Rust Mail protocol request was rejected.');
    return result.result;
  }

  async call(name: string, args: Record<string, unknown>): Promise<any> {
    const deadline = Date.now() + this.requestBudgetMs;
    this.initialized ??= (async () => {
      await this.rpc('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'crewroom', version: '0.1.0' } }, false, deadline);
      await this.rpc('notifications/initialized', {}, true, deadline);
    })().catch(error => { this.initialized = undefined; throw error; });
    await this.initialized;
    // Concurrent runs share one Rust Mail database. It reports a busy database as a recoverable,
    // retry-safe tool error: the statement did not commit, so a bounded retry repeats nothing.
    for (let attempt = 1; ; attempt++) {
      let response;
      try { response = await this.rpc('tools/call', { name, arguments: args }, false, deadline); }
      catch (error) {
        // Timeout consumes the read budget and returns to supervision immediately.
        // Reads can reconnect after service loss. A keyed send can retry an explicit rate-limit
        // rejection; ambiguous writes stay pending for domain reconciliation, never blind replay.
        const read = /^(fetch_|get_)/.test(name);
        const safeRateLimit = read || name === 'send_message' && typeof args.idempotency_key === 'string';
        const transient = error instanceof MailHttpError
          ? error.status === 429 && safeRateLimit || error.status === 503 && read
          : read && error instanceof TypeError;
        const wait = Math.max(100 * 2 ** (attempt - 1), error instanceof MailHttpError ? error.retryAfter : 0);
        if (!transient || attempt >= 5 || wait > 5000 || Date.now() + wait >= deadline) throw error;
        await this.pause(wait); continue;
      }
      if (!response.isError) return response.structuredContent ?? JSON.parse(response.content.filter((part: any) => part.type === 'text').map((part: any) => part.text).join('\n'));
      const reason = (response.content ?? []).filter((part: { type?: string }) => part.type === 'text').map((part: { text?: string }) => part.text ?? '').join(' ');
      let retrySafe = false;
      try { const error = JSON.parse(reason).error; retrySafe = error?.recoverable === true && error?.data?.db_error_classification?.safe_to_retry === true; } catch {}
      if (!retrySafe || attempt >= 5) throw new Error(`Rust Mail rejected ${name}${reason ? `: ${reason.slice(0, 300)}` : '.'}`);
      const wait = 100 * 2 ** (attempt - 1);
      if (Date.now() + wait >= deadline) throw new DOMException('Rust Mail request budget exhausted.', 'TimeoutError');
      await this.pause(wait);
    }
  }
}

export class Messaging {
  private nextObservation = 0;
  constructor(private store: Workbench, private mail: RustMailClient, private project: string, private retentionSeconds: number, private now = () => Date.now()) {}

  async register(runId: string) {
    await this.mail.call('ensure_project', { human_key: this.project });
    // Exact names characterized against Rust Mail 0.3.36; never reuse an old identity within a run.
    const names = [...new Set(['BlueLake', 'GreenCastle', 'RedPine', 'SilverHill', 'AmberCreek',
      ...['Blue', 'Green', 'Red', 'Silver', 'Amber', 'White', 'Brown', 'Orange', 'Purple', 'Golden'].flatMap(adjective =>
        ['Lake', 'Castle', 'Pine', 'Hill', 'Creek', 'Mountain', 'Dog', 'Stone', 'River', 'Forest'].map(noun => adjective + noun))])];
    const assignments = this.store.runtime(runId).assignments;
    for (const [index, assignment] of assignments.entries()) {
      if (assignment.retiredAt) continue;
      const name = assignment.mailboxName ?? names[index];
      if (!name) throw new Error('This run has exhausted the 100 verified Rust Mail identities; start a separate run.');
      // Bind the deterministic name before calling the external service, so a
      // lost registration response cannot create another identity on restart.
      this.store.bindMailbox(assignment.id, name);
      const registered = await this.mail.call('register_agent', { project_key: this.project, name, program: 'omp', model: attemptModel(assignment, this.store.getAttempt(assignment.currentAttemptId)).resolved.selector });
      if (registered.name !== name) throw new Conflict('Mailbox registration identity mismatch.');
      await this.mail.call('set_contact_policy', { project_key: this.project, agent_name: name, policy: 'open' });
    }
  }

  async send(attemptId: string, toolCallId: string, input: unknown) {
    const message = this.store.prepareMessage(attemptId, toolCallId, input);
    return this.reconcile(message.id);
  }

  async reconcile(id: string) {
    const message = this.store.getPeerMessage(id);
    if (message.transportId) return message;
    const assignments = this.store.runtime(message.runId).assignments;
    const sender = assignments.find(item => item.id === message.from)!;
    const recipient = assignments.find(item => item.id === message.to)!;
    if (!sender.mailboxName || !recipient.mailboxName) throw new Conflict('Mailboxes are not registered.');
    if (this.now() - Date.parse(message.createdAt) >= this.retentionSeconds * 1000) {
      const inbox = await this.mail.call('fetch_inbox', { project_key: this.project, agent_name: recipient.mailboxName, include_bodies: true, mark_read: false, limit: 100 });
      const matches = z.array(z.object({ id: z.number(), subject: z.string(), from: z.string(), body_md: z.string() })).parse(inbox)
        .filter(item => item.subject === message.id && item.from === sender.mailboxName && item.body_md === message.body);
      if (matches.length !== 1) {
        this.store.markMessageUncertain(message.id);
        throw new Conflict('Message retry key expired; storage outcome remains uncertain. Do not resend.');
      }
      return this.store.recordMessageStored(message.id, matches[0].id);
    }
    const result = await this.mail.call('send_message', { project_key: this.project, sender_name: sender.mailboxName,
      to: [recipient.mailboxName], subject: message.id, body_md: message.body, ack_required: true,
      thread_id: message.runId, idempotency_key: message.id });
    const transport = z.object({ count: z.literal(1), deliveries: z.array(z.object({ payload: z.object({ id: z.number().int().positive() }) })).length(1) }).parse(result);
    return this.store.recordMessageStored(message.id, transport.deliveries[0].payload.id);
  }

  async accept(id: string) {
    // The saved body is the recoverable input, independent of OMP's session file.
    const message = this.store.acceptMessage(id);
    const recipient = this.store.runtime(message.runId).assignments.find(item => item.id === message.to)!;
    await this.mail.call('acknowledge_message', { project_key: this.project, agent_name: recipient.mailboxName, message_id: message.transportId });
    return message;
  }

  async observe(runId: string) {
    if (this.now() < this.nextObservation) return [];
    // Host events may arrive in bursts. External observation is passive and bounded even then.
    this.nextObservation = this.now() + 1500;
    const collected: { cursor: number; message_id: number; [key: string]: unknown }[] = [];
    for (const assignment of this.store.runtime(runId).assignments) {
      if (!assignment.mailboxName) continue;
      let more = true;
      while (more) {
        const after = this.store.observerCursor(assignment.id);
        const page = z.object({ events: z.array(z.object({ cursor: z.number(), message_id: z.number() }).passthrough()), next_cursor: z.number(), has_more: z.boolean() })
          .parse(await this.mail.call('fetch_inbox_events', { project_key: this.project, agent_name: assignment.mailboxName, after, limit: 100 }));
        if (page.has_more && page.next_cursor <= after) throw new Error('Mailbox observer made no progress.');
        this.store.recordObserverPage(assignment.id, page.events, page.next_cursor);
        collected.push(...page.events); more = page.has_more;
      }
    }
    return collected;
  }
}
