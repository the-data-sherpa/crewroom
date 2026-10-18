import { useState } from 'react';
import type { Workspace } from '../shared/types';
import type { LearningProposal } from '../shared/learning';
import type { Action } from './main';

/** Coordinator-proposed learning updates. Only the operator's decision applies one, as a new version for future assignments. */
export function LearningPanel({ workspace, action, busy }: { workspace: Workspace; action: Action; busy: boolean }) {
  const pending = workspace.learning.filter(item => item.status === 'pending');
  const decided = workspace.learning.filter(item => item.status !== 'pending').reverse();
  return <>
    <p className="muted">Proposals cite a run's recorded evidence. Approving one saves a new version of the role or project guidance for future assignments; admitted assignments, models, checks and readiness rules do not change.</p>
    {!workspace.learning.length ? <p>No learning proposals recorded.</p> : !pending.length && <p>No proposals are waiting for a decision.</p>}
    {pending.map(proposal => <Proposal key={proposal.id} proposal={proposal} workspace={workspace} action={action} busy={busy} />)}
    {!!decided.length && <details><summary>Decided proposals ({decided.length})</summary>{decided.map(proposal => <Proposal key={proposal.id} proposal={proposal} workspace={workspace} action={action} busy={busy} />)}</details>}
  </>;
}

function Proposal({ proposal, workspace, action, busy }: { proposal: LearningProposal; workspace: Workspace; action: Action; busy: boolean }) {
  const [reason, setReason] = useState('');
  const run = workspace.runs.find(item => item.id === proposal.runId);
  const target = proposal.target;
  const role = target.kind === 'role' ? workspace.roles.find(item => item.id === target.roleId) : undefined;
  const repository = target.kind === 'project' ? workspace.repositories.find(item => item.id === target.repositoryId) : undefined;
  const title = target.kind === 'role' ? `Role · ${role?.name ?? target.roleId}` : `Project guidance · ${repository?.name ?? 'repository'}`;
  const current = target.kind === 'role' ? role?.instructions : repository?.guidance?.at(-1)?.text;
  const decide = (decision: 'approve' | 'reject') => action(`/learning/${proposal.id}/decision`, { decision, reason, requestId: crypto.randomUUID() },
    decision === 'approve' ? 'Learning update approved. Future assignments use the new version.' : 'Learning proposal rejected.');
  return <article className="learning" aria-label={title}>
    <div className="section-title"><strong>{title}</strong><span className={`state-pill ${proposal.status === 'approved' && proposal.decision?.appliedVersion ? 'validated' : ''}`}>{proposal.status === 'pending' ? `Proposed against version ${proposal.baseVersion}`
      : proposal.status === 'approved' ? proposal.decision?.appliedVersion ? `Approved · version ${proposal.decision.appliedVersion}` : 'Approved · saving new version'
        : proposal.status === 'superseded' ? 'Approved · not applied' : 'Rejected'}</span></div>
    {proposal.decision?.notApplied && <p className="callout">{proposal.decision.notApplied}</p>}
    <p className="muted">From run of {run?.taskSnapshot.title ?? 'an earlier ticket'} · {new Date(proposal.createdAt).toLocaleString()}</p>
    <p>{proposal.rationale}</p>
    <p className="muted">Evidence: {proposal.evidence.map(item => `${item.kind} ${item.id.slice(0, 8)}`).join(', ')}</p>
    <details open={proposal.status === 'pending'}><summary>Proposed text</summary><pre className="evidence-content">{proposal.change}</pre>
      {proposal.status === 'pending' && <><p className="muted">Current version:</p><pre className="evidence-content">{current ?? 'None'}</pre></>}</details>
    {proposal.decision && <p className="muted">Decision: {proposal.decision.reason}</p>}
    {proposal.status === 'pending' && <fieldset disabled={busy}>
      <label>Decision reason<textarea value={reason} onChange={event => setReason(event.target.value)} rows={2} maxLength={2000} required /></label>
      <div className="form-actions"><button className="primary" disabled={!reason.trim()} onClick={() => void decide('approve')}>Approve</button><button disabled={!reason.trim()} onClick={() => void decide('reject')}>Reject</button></div>
    </fieldset>}
    {proposal.status === 'approved' && proposal.decision?.pendingRoleSave && <button disabled={busy} onClick={() => void action(`/learning/${proposal.id}/decision`, { decision: 'approve', reason: proposal.decision!.reason, requestId: proposal.decision!.requestId }, 'Approved role version saved.')}>Retry saving the approved version</button>}
  </article>;
}
