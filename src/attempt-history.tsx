import { useEffect, useState } from 'react';
import type { RunHistory, AttemptEvidence } from '../server/run-history';
import type { Run } from '../shared/types';
import { ticketHref, type RunTab } from './navigation';
import { UsageNumbers } from './usage';
const time = (value: string | null | undefined) => value ? new Date(value).toLocaleString() : 'Unknown · not recorded';

function EvidenceRecord({ item, runId }: { item: AttemptEvidence; runId: string }) {
  const [content, setContent] = useState<string>(); const [error, setError] = useState(''); const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open || !item.artifact) return;
    let active = true;
    void fetch(`/api/runs/${encodeURIComponent(runId)}/result/artifacts/${encodeURIComponent(item.artifact.id)}`).then(async response => {
      const result = await response.json(); if (!response.ok) throw new Error(result.error ?? 'Evidence unavailable.');
      if (active) setContent(result.content);
    }).catch(cause => { if (active) setError(cause instanceof Error ? cause.message : 'Evidence unavailable.'); });
    return () => { active = false; };
  }, [open, runId, item.artifact?.id]);
  return <li><h4>{item.label}</h4><p className="hash">{item.revision}</p><p className="muted">Recorded {time(item.at)}</p>
    {item.detail && <p>{item.detail}</p>}
    {item.artifact && <><p className="hash">Artifact SHA-256: {item.artifact.sha256}</p><button onClick={() => setOpen(!open)} aria-expanded={open}>{open ? 'Hide' : 'View'} saved artifact</button>
      {open && (error ? <p role="alert" className="error">{error}</p> : content === undefined ? <p>Loading artifact…</p> : <pre className="evidence-content" aria-label="Saved attempt evidence">{content}</pre>)}</>}
  </li>;
}

export function AttemptHistory({ run, history, selectedId, tab }: { run: Run; history: RunHistory; selectedId?: string; tab: RunTab }) {
  const selected = history.attempts.find(item => item.attemptId === selectedId);
  return <section className="attempt-history" aria-label="Attempt history">
    <h2>Attempt history</h2>
    <nav aria-label="Attempts" className="history-strip">{history.attempts.map(item => <a key={item.attemptId} className="chip"
      href={ticketHref({ taskId: run.taskId, runId: run.id, attemptId: item.attemptId, tab })} aria-current={item === selected ? 'page' : undefined}>
      {item.name} · attempt {item.generation}{item.retiredAt ? ' · retired' : item.current ? ' · current' : ' · prior'} · {item.state === 'exited' ? 'stopped' : item.state.replaceAll('_', ' ')}</a>)}</nav>
    {selectedId && !selected && <p role="status" className="callout">That attempt is unavailable in this run. Showing the agent wall.</p>}
    {selected && <article className="attempt-detail" aria-label={`${selected.name} attempt ${selected.generation} details`}>
      <div className="section-title"><h3>{selected.name} · attempt {selected.generation}</h3><a href={ticketHref({ taskId: run.taskId, runId: run.id, tab })}>Back to agent wall</a></div>
      <p className="muted">Read-only {selected.current ? 'current' : 'prior'} attempt. This view does not start or resume an agent.</p>
      <p>Model: {selected.model.resolved.selector} · {selected.model.resolved.effort}</p>
      <p>Recorded state: {selected.state === 'exited' ? 'stopped' : selected.state.replaceAll('_', ' ')} · Ownership: {selected.ownership ?? 'unclaimed'}</p>
      {selected.retiredAt && <p>Assignment retired {time(selected.retiredAt)}: {selected.retirementReason ?? 'reason unknown; not recorded'}</p>}
      <h4>Recorded cause</h4>{selected.causes.length ? selected.causes.map(cause => <p key={cause}>{cause}</p>) : <p>No failure or stop recorded.</p>}
      <p>Next: {selected.nextAction}</p>
      <dl><dt>Admitted</dt><dd>{time(selected.createdAt)}</dd><dt>Start verified</dt><dd>{time(selected.startedAt)}</dd><dt>Stop confirmed</dt><dd>{time(selected.stoppedAt)}</dd></dl>
      <section aria-label="Attempt usage"><h4>Recorded usage</h4><UsageNumbers usage={history.usage.rows.find(row => row.attemptId === selected.attemptId)!} />
        {history.usage.rows.find(row => row.attemptId === selected.attemptId)?.unavailableReasons.map(reason => <p key={reason}>{reason}</p>)}
        <p className="muted">Source: {history.usage.source}.</p>
      </section>
      <h4>Evidence produced</h4><p className="muted">These records cover their named revisions. They do not establish current readiness.</p>
      {!selected.evidence.length ? <p>No evidence recorded for this attempt.</p> : <ul className="attempt-evidence">{selected.evidence.map(item => <EvidenceRecord key={item.id} item={item} runId={run.id} />)}</ul>}
    </article>}
  </section>;
}
