import { useState } from 'react';
import type { Task, Workspace } from '../shared/types';
import type { RefinementProposal } from '../shared/refinement';
import type { Action } from './main';

function Decision({ proposal, action, busy, stale }: { proposal: RefinementProposal; action: Action; busy: boolean; stale: boolean }) {
  const [editing, setEditing] = useState(false);
  const [brief, setBrief] = useState(proposal.brief);
  const [criteria, setCriteria] = useState(proposal.criteria.join('\n'));
  const [reason, setReason] = useState('');
  const decide = async (decision: 'accept' | 'discard') => {
    const result = await action(`/refinement-proposals/${proposal.id}/decision`, {
      decision, requestId: crypto.randomUUID(), ...(reason.trim() ? { reason: reason.trim() } : {}),
      ...(decision === 'accept' && editing ? { brief, criteria: criteria.split('\n').map(item => item.trim()).filter(Boolean) } : {}),
    }, decision === 'accept' ? 'Refinement accepted. Run snapshots stay unchanged until adopted.' : 'Proposal discarded.');
    if (result) setReason('');
  };
  return <form onSubmit={event => { event.preventDefault(); void decide('accept'); }}>
    <label><input type="checkbox" checked={editing} onChange={event => setEditing(event.target.checked)} disabled={busy} /> Edit before accepting</label>
    {editing && <><label>Proposed brief<textarea value={brief} onChange={event => setBrief(event.target.value)} required rows={4} /></label>
      <label>Proposed criteria<textarea value={criteria} onChange={event => setCriteria(event.target.value)} required rows={3} /></label></>}
    <label>Decision reason (required to discard)<input value={reason} onChange={event => setReason(event.target.value)} /></label>
    <div className="form-actions"><button className="primary" disabled={busy || stale || (editing && (!brief.trim() || !criteria.trim()))}>Accept proposal</button>
      <button type="button" disabled={busy || !reason.trim()} onClick={() => void decide('discard')}>Discard proposal</button></div>
  </form>;
}

export function RefinementPanel({ task, workspace, action, busy }: { task: Task; workspace: Workspace; action: Action; busy: boolean }) {
  const proposals = workspace.refinementProposals.filter(item => item.taskId === task.id).toReversed();
  const notes = workspace.notes.filter(item => item.taskId === task.id).toReversed();
  const proposalStatus: Record<string, RefinementProposal['status']> = Object.fromEntries(proposals.map(item => [item.id, item.status]));
  if (!proposals.length && !notes.length) return null;
  return <section className="card" aria-label="Refinement proposals and notes"><h2>Explore &amp; Grill history</h2>
    {proposals.map(proposal => <article key={proposal.id} className="proposal"><h3>{workspace.runs.find(item => item.id === proposal.runId)?.purpose === 'grill' ? 'Grill' : 'Explore'} proposal · {proposal.status}</h3>
      <p className="muted">Based on accepted revision {proposal.baseRevision} · source revision {proposal.sourceRevision}{proposal.decision?.revision ? ` · decision revision ${proposal.decision.revision}` : ''}</p>
      <p>{proposal.brief}</p><ul>{proposal.criteria.map((criterion, index) => <li key={index}>{criterion}</li>)}</ul>
      {proposal.decisions.length > 0 && <><strong>Decisions</strong><ul>{proposal.decisions.map((decision, index) => <li key={index}>{decision}</li>)}</ul></>}
      {proposal.unresolvedQuestions.length > 0 && <><strong>Unresolved questions</strong><ul>{proposal.unresolvedQuestions.map((question, index) => <li key={index}>{question}</li>)}</ul></>}
      {proposal.status === 'pending' && <Decision proposal={proposal} action={action} busy={busy} stale={task.revision !== proposal.baseRevision || task.sourceRevision !== proposal.sourceRevision || !!task.needsReacceptance} />}
      {proposal.status === 'pending' && (task.revision !== proposal.baseRevision || task.sourceRevision !== proposal.sourceRevision || task.needsReacceptance) && <p className="callout">Proposal base is stale. Reload and request a new proposal before accepting.</p>}
      {proposal.decision?.reason && <p>Operator decision: {proposal.decision.reason}</p>}
    </article>)}
    <h3>Saved notes</h3>{notes.length === 0 && <p className="muted">No notes yet.</p>}
    <ul>{notes.map(note => <li key={note.id}><strong>{note.kind === 'unresolved_question' ? 'Unresolved question' : note.mode}{note.proposalId && ` · ${proposalStatus[note.proposalId] ?? 'historical'} proposal`}</strong>{note.proposalId && ` · proposal ${note.proposalId.slice(0, 8)} · revision ${note.revision}`} — {note.body}</li>)}</ul>
  </section>;
}
