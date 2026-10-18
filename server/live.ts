import { createHash } from 'node:crypto';
import type { IncomingMessage, Server } from 'node:http';
import { join } from 'node:path';
import { WebSocketServer, type WebSocket } from 'ws';
import { z } from 'zod';
import type { Workbench } from './workbench';
import { TerminalHub, followFile, PANE_COLUMNS, PANE_ROWS, REPLAY_BYTES, tail } from './terminals';

/**
 * One push channel per browser tab. Server-side changes (store commits, plus a slow
 * heartbeat for in-memory supervisor facts) are pushed only when a subscribed view
 * changed; the browser does not poll. Terminal topics stream read-only pane output.
 */
export interface LiveSources {
  store: Workbench;
  workspace: () => unknown;
  run: (runId: string) => unknown;
  result: (runId: string) => Promise<unknown>;
  runtimeRoot?: string;
  terminals?: TerminalHub;
}

const message = z.discriminatedUnion('op', [
  z.object({ op: z.literal('subscribe'), topic: z.string().max(300), after: z.number().int().min(0).optional() }).strict(),
  z.object({ op: z.literal('unsubscribe'), topic: z.string().max(300) }).strict(),
]);
const HIGH_WATER = 2 * 1024 * 1024;
const LOW_WATER = 256 * 1024;
const hash = (value: string) => createHash('sha256').update(value).digest('hex');


interface Subscription { topic: string; sent?: string; cursor?: number; stop?: () => void; lagging?: boolean; resync?: () => Promise<void> }

export function checkKey(toolCallId: string) { return createHash('sha256').update(toolCallId).digest('hex'); }

export class LiveHub {
  private wss = new WebSocketServer({ noServer: true, maxPayload: 16 * 1024 });
  private clients = new Map<WebSocket, Map<string, Subscription>>();
  private timer: ReturnType<typeof setInterval>;
  private scheduled?: ReturnType<typeof setTimeout>;
  private evaluating = false; private again = false;
  private unsubscribeStore: () => void;

  constructor(server: Server, private sources: LiveSources, private allowed: (req: IncomingMessage) => boolean) {
    server.on('upgrade', (req, socket, head) => {
      if (!req.url?.startsWith('/api/live')) return;
      if (!this.allowed(req)) { socket.write('HTTP/1.1 403 Forbidden\r\n\r\n'); socket.destroy(); return; }
      this.wss.handleUpgrade(req, socket, head, ws => this.connect(ws));
    });
    this.unsubscribeStore = sources.store.onChange(() => this.schedule());
    this.timer = setInterval(() => this.schedule(), 2000); this.timer.unref();
  }

  private connect(ws: WebSocket) {
    const subs = new Map<string, Subscription>(); this.clients.set(ws, subs);
    ws.on('message', raw => {
      let parsed; try { parsed = message.parse(JSON.parse(String(raw))); } catch { this.send(ws, { topic: '', kind: 'error', error: 'Invalid live request.' }); return; }
      if (parsed.op === 'unsubscribe') { subs.get(parsed.topic)?.stop?.(); subs.delete(parsed.topic); return; }
      if (subs.has(parsed.topic)) subs.get(parsed.topic)!.stop?.();
      const sub: Subscription = { topic: parsed.topic, cursor: parsed.after };
      subs.set(parsed.topic, sub);
      void this.start(ws, sub);
    });
    ws.on('close', () => { for (const sub of subs.values()) sub.stop?.(); this.clients.delete(ws); });
    ws.on('error', () => ws.terminate());
  }

  private send(ws: WebSocket, value: unknown) { if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(value)); }

  private async start(ws: WebSocket, sub: Subscription) {
    const [kind, id, extra] = sub.topic.split(':');
    try {
      if (kind === 'terminal' && id) return await this.terminal(ws, sub, id);
      if (kind === 'check' && id && extra) return await this.check(ws, sub, id, extra);
      await this.refresh(ws, sub);
    } catch (error) { this.send(ws, { topic: sub.topic, kind: 'error', error: error instanceof Error ? error.message : 'Unavailable.' }); }
  }

  private async value(topic: string): Promise<unknown> {
    const [kind, id] = topic.split(':');
    if (topic === 'workspace') return this.sources.workspace();
    if (kind === 'run' && id) return this.sources.run(id);
    if (kind === 'result' && id) return this.sources.result(id);
    throw new Error('Unknown live topic.');
  }

  private async refresh(ws: WebSocket, sub: Subscription) {
    const [kind, id] = sub.topic.split(':');
    if (kind === 'events' && id) {
      for (;;) {
        const page = this.sources.store.agentEventPage(id, sub.cursor ?? 0, 200);
        if (page.events.length) this.send(ws, { topic: sub.topic, kind: 'events', events: page.events, after: page.after });
        sub.cursor = page.after;
        if (!page.hasMore) break;
      }
      return;
    }
    const text = JSON.stringify(await this.value(sub.topic)); const digest = hash(text);
    if (digest === sub.sent) return;
    sub.sent = digest;
    if (ws.readyState === ws.OPEN) ws.send(`{"topic":${JSON.stringify(sub.topic)},"kind":"state","data":${text}}`);
  }

  private attempt(attemptId: string) {
    return this.sources.store.getAttempt(attemptId);
  }

  private stream(ws: WebSocket, sub: Subscription, source: () => Promise<{ data: Buffer; source: string }>, live: boolean) {
    const reset = async () => {
      const snapshot = await source();
      this.send(ws, { topic: sub.topic, kind: 'reset', cols: PANE_COLUMNS, rows: PANE_ROWS, source: snapshot.source, live, data: snapshot.data.toString('base64') });
    };
    sub.resync = async () => { sub.lagging = false; await reset(); };
    return { reset, push: (chunk: Buffer) => {
      if (sub.lagging) return;
      if (ws.bufferedAmount > HIGH_WATER) { sub.lagging = true; this.send(ws, { topic: sub.topic, kind: 'lagging' }); return; }
      this.send(ws, { topic: sub.topic, kind: 'data', data: chunk.toString('base64') });
    } };
  }

  private async terminal(ws: WebSocket, sub: Subscription, attemptId: string) {
    const hub = this.sources.terminals;
    const attempt = this.attempt(attemptId);
    if (!hub) { this.send(ws, { topic: sub.topic, kind: 'unavailable', reason: 'Runner integration is unavailable.' }); return; }
    const pane = attempt.pane;
    const live = !!pane && attempt.ownership !== 'stopped' && attempt.state !== 'exited';
    const { reset, push } = this.stream(ws, sub, () => hub.snapshot(pane, attemptId), live);
    // Subscribe before the snapshot so output between them is not lost (it may repeat briefly).
    if (pane) sub.stop = hub.subscribe(pane, push);
    await reset();
  }

  private async check(ws: WebSocket, sub: Subscription, attemptId: string, toolCallId: string) {
    const root = this.sources.runtimeRoot;
    this.attempt(attemptId);
    if (!root) { this.send(ws, { topic: sub.topic, kind: 'unavailable', reason: 'Runner integration is unavailable.' }); return; }
    const path = join(root, attemptId, 'tools', checkKey(toolCallId), 'live.log');
    const follower = followFile(path, chunk => push(chunk));
    let first = true;
    const { reset, push } = this.stream(ws, sub, async () => {
      const data = first ? follower.initial : tail(path, REPLAY_BYTES);
      first = false;
      return { data, source: 'log' };
    }, true);
    sub.stop = follower.stop;
    await reset();
  }

  /** Re-evaluate subscribed views soon (after mutations that do not commit to the store, e.g. role files). */
  changed() { this.schedule(); }

  private schedule() {
    if (this.scheduled) return;
    this.scheduled = setTimeout(() => { this.scheduled = undefined; void this.evaluate(); }, 75);
  }

  private async evaluate() {
    if (this.evaluating) { this.again = true; return; }
    this.evaluating = true;
    try {
      for (const [ws, subs] of this.clients) {
        for (const sub of subs.values()) {
          const kind = sub.topic.split(':')[0];
          if (kind === 'terminal' || kind === 'check') {
            if (sub.lagging && ws.bufferedAmount < LOW_WATER) await sub.resync?.().catch(() => {});
            continue;
          }
          try { await this.refresh(ws, sub); }
          catch (error) { this.send(ws, { topic: sub.topic, kind: 'error', error: error instanceof Error ? error.message : 'Unavailable.' }); }
        }
      }
    } finally {
      this.evaluating = false;
      if (this.again) { this.again = false; this.schedule(); }
    }
  }

  close() {
    clearInterval(this.timer); clearTimeout(this.scheduled); this.unsubscribeStore();
    for (const [ws, subs] of this.clients) { for (const sub of subs.values()) sub.stop?.(); ws.terminate(); }
    this.clients.clear(); this.wss.close();
  }
}
