/** Streaming frames repeat cumulative text/tool arguments. They are activity, not receipts. */
export function compactObservation(event: Record<string, unknown>): Record<string, unknown> {
  if (event.type !== 'message_update') return event;
  const delta = event.assistantMessageEvent && typeof event.assistantMessageEvent === 'object'
    ? event.assistantMessageEvent as Record<string, unknown> : {};
  const phase = event.phase ?? delta.type;
  const index = event.contentIndex ?? delta.contentIndex;
  return { type: 'message_update', messageId: typeof event.messageId === 'string' ? event.messageId.slice(0, 200) : undefined,
    phase: typeof phase === 'string' ? phase.slice(0, 80) : 'streaming', contentIndex: Number.isSafeInteger(index) ? index : undefined };
}

/** Keep visible progress at most four times a second; preserve every completed frame. */
export class ObservationStream {
  private messageId: unknown;
  private at = -Infinity;
  constructor(private now: () => number = Date.now) {}
  accept(event: Record<string, unknown>) {
    if (event.type !== 'message_update') return event;
    const now = this.now();
    if (this.messageId === event.messageId && now - this.at < 250) return undefined;
    this.messageId = event.messageId; this.at = now;
    return compactObservation(event);
  }
}
