import type { Workspace } from '../shared/types';
import type { Action } from './main';

export function JiraSourcePanel({ workspace, action, busy }: { workspace: Workspace; action: Action; busy: boolean }) {
  const jira = workspace.jira;
  if (!jira) return null;
  return <section className="card jira-source" aria-label="Jira source">
    <div className="section-title"><h2>{jira.source.name}</h2><button disabled={busy} onClick={() => action('/jira/sync', {}, 'Jira refresh finished. See connection status below.')}>Refresh Jira</button></div>
    <p>Filter {jira.source.filterId} · Assignee {jira.source.assigneeAccountId} · Rank ascending</p>
    <p>{jira.state === 'not_synced' ? 'Not refreshed yet' : jira.state === 'connected' ? `${jira.taskIds.length} tickets in the source` : 'Jira unavailable — showing saved work'}</p>
    {jira.lastSyncedAt && <small>Last complete refresh: {new Date(jira.lastSyncedAt).toLocaleString()}</small>}
    {jira.error && <p role="alert">{jira.error}</p>}
  </section>;
}
export function JiraUpdates({ workspace, runId, action, busy }: { workspace: Workspace; runId: string; action: Action; busy: boolean }) {
  const updates = workspace.jiraUpdates?.filter(item => item.runId === runId) ?? [];
  if (!workspace.jira && !updates.length) return null;
  return <section className="card" aria-label="Jira delivery">
    <div className="section-title"><h2>Jira delivery</h2><button disabled={busy || !workspace.jira || !updates.some(item => ['pending', 'uncertain', 'sending'].includes(item.state))} onClick={() => action('/jira/reconcile', {}, 'Jira delivery receipts refreshed.')}>Reconcile updates</button></div>
    <p className="muted">Only confirmed updates have provider receipts. Jira status is refreshed separately.</p>
    {!updates.length && <p>No Jira updates requested.</p>}
    {updates.map(update => <article key={update.id}><strong>{update.kind.replace('_', ' ')} · {update.state === 'sending' ? 'Sending — confirmation pending' : update.state}</strong><p>{update.body ?? 'Configured In Progress transition'}</p><small>{update.eventKey}{update.externalId && ` · Receipt ${update.externalId}`}</small>{update.reason && <p>{update.reason}</p>}</article>)}
  </section>;
}
