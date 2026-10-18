import { useSyncExternalStore } from 'react';
export type RunTab = 'coordinator' | 'evidence' | 'brief';
export interface TicketLocation { taskId: string; runId?: string; attemptId?: string; tab?: RunTab }

export function parseTicketLocation(hash: string): TicketLocation {
  const value = hash.replace(/^#/, ''); const split = value.indexOf('?');
  const rawTask = split < 0 ? value : value.slice(0, split);
  const query = new URLSearchParams(split < 0 ? '' : value.slice(split + 1));
  let taskId: string; try { taskId = decodeURIComponent(rawTask); } catch { taskId = rawTask; }
  const tab = query.get('tab');
  return { taskId, runId: query.get('run') || undefined, attemptId: query.get('attempt') || undefined,
    tab: tab === 'coordinator' || tab === 'evidence' || tab === 'brief' ? tab : undefined };
}
export function ticketHref(value: TicketLocation) {
  const query = new URLSearchParams();
  if (value.runId) query.set('run', value.runId);
  if (value.attemptId) query.set('attempt', value.attemptId);
  if (value.tab) query.set('tab', value.tab);
  return `#${encodeURIComponent(value.taskId)}${query.size ? `?${query}` : ''}`;
}
const subscribe = (notify: () => void) => { addEventListener('hashchange', notify); return () => removeEventListener('hashchange', notify); };
export function useTicketLocation() { return parseTicketLocation(useSyncExternalStore(subscribe, () => location.hash)); }
export function navigateTicket(value: TicketLocation, replace = false) {
  const hash = ticketHref(value); if (location.hash === hash) return;
  if (replace) { history.replaceState(null, '', hash); dispatchEvent(new HashChangeEvent('hashchange')); }
  else location.hash = hash;
}
