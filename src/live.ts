import { useEffect, useState, useSyncExternalStore } from 'react';

/**
 * The browser's single push connection. State topics deliver a full view only when it
 * changed; event topics deliver new events after a cursor; terminal topics stream
 * read-only pane output. Nothing here polls: reconnects resume from saved cursors.
 */
export type LiveMessage = { topic: string; kind: string; [key: string]: any };
type Handler = (message: LiveMessage) => void;

class LiveConnection {
  private ws?: WebSocket;
  private handlers = new Map<string, Set<Handler>>();
  private cursors = new Map<string, number>();
  private latest = new Map<string, LiveMessage>();
  private statusListeners = new Set<() => void>();
  private delay = 500;
  private timer?: ReturnType<typeof setTimeout>;
  status: 'connecting' | 'open' | 'closed' = 'connecting';
  everOpened = false;

  private setStatus(status: LiveConnection['status']) { this.status = status; for (const listener of this.statusListeners) listener(); }
  onStatus(listener: () => void) { this.statusListeners.add(listener); return () => { this.statusListeners.delete(listener); }; }

  private connect() {
    if (this.ws || typeof WebSocket === 'undefined') return;
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/live`);
    this.ws = ws;
    ws.onopen = () => {
      this.delay = 500; this.everOpened = true; this.setStatus('open');
      for (const topic of this.handlers.keys()) this.request(topic);
    };
    ws.onmessage = event => {
      let message: LiveMessage; try { message = JSON.parse(String(event.data)); } catch { return; }
      if (message.kind === 'events' && typeof message.after === 'number') this.cursors.set(message.topic, message.after);
      if (message.kind === 'state') this.latest.set(message.topic, message);
      for (const handler of this.handlers.get(message.topic) ?? []) handler(message);
    };
    ws.onclose = () => {
      this.ws = undefined; this.setStatus('closed');
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.connect(), this.delay);
      this.delay = Math.min(this.delay * 2, 5000);
    };
    ws.onerror = () => ws.close();
  }

  private request(topic: string) {
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    const after = this.cursors.get(topic);
    this.ws.send(JSON.stringify({ op: 'subscribe', topic, ...(after !== undefined ? { after } : {}) }));
  }

  /** `restart` asks the server for a fresh snapshot even if the topic is already subscribed (terminals). */
  subscribe(topic: string, handler: Handler, options: { restart?: boolean } = {}) {
    this.connect();
    const set = this.handlers.get(topic) ?? new Set(); const first = !set.size;
    set.add(handler); this.handlers.set(topic, set);
    const cached = this.latest.get(topic);
    if (cached) handler(cached);
    if (first || options.restart) this.request(topic);
    return () => {
      set.delete(handler);
      if (!set.size) {
        this.handlers.delete(topic); this.latest.delete(topic); this.cursors.delete(topic);
        if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ op: 'unsubscribe', topic }));
      }
    };
  }
}

export const live = new LiveConnection();

/** True once connected and while the connection is open. False during reconnects. */
export function useLiveStatus() {
  return useSyncExternalStore(listener => live.onStatus(listener), () => live.status);
}

export function useTopic<T>(topic: string | undefined): { data?: T; error?: string } {
  const [state, setState] = useState<{ topic?: string; data?: T; error?: string }>({});
  useEffect(() => {
    if (!topic) return;
    return live.subscribe(topic, message => {
      if (message.kind === 'state') setState({ topic, data: message.data as T });
      else if (message.kind === 'error') setState(current => ({ ...current, topic, error: message.error }));
    });
  }, [topic]);
  return state.topic === topic ? state : {};
}

export interface LiveEvent { cursor: number; attemptId: string; sequence: number; event: Record<string, any> }

/** Every event for a run, delivered once each, resumed from the last cursor after reconnect. */
export function useRunEvents(runId: string, keep: (event: LiveEvent) => boolean) {
  const [events, setEvents] = useState<LiveEvent[]>([]);
  useEffect(() => {
    setEvents([]);
    return live.subscribe(`events:${runId}`, message => {
      if (message.kind !== 'events') return;
      const kept = (message.events as LiveEvent[]).filter(keep);
      if (kept.length) setEvents(previous => {
        const seen = new Set(previous.map(item => item.cursor));
        return [...previous, ...kept.filter(item => !seen.has(item.cursor))];
      });
    });
    // `keep` is a stable module-level predicate at every call site.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId]);
  return events;
}

export const decodeBase64 = (value: string) => Uint8Array.from(atob(value), char => char.charCodeAt(0));
