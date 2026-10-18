import { useRef } from 'react';
import type { Role, Run } from '../shared/types';
import type { Assignment, TeamProposal } from '../server/runtime/types';
import type { Action } from './main';

export function SpecialistInsertion({ run, roles, assignments, proposals, disabledReason, action }: {
  run: Run; roles: Role[]; assignments: Assignment[]; proposals: TeamProposal[];
  disabledReason?: string; action: Action;
}) {
  const request = useRef<{ content: string; id: string } | undefined>(undefined);
  const saving = useRef(false);
  const available = roles.filter(role => role.enabled && !(run.purpose === 'research' && ['implementer', 'reviewer'].includes(role.id)));
  const team = assignments.length ? assignments : run.launch?.team ?? [{ name: 'Coordinator', coordinator: true }];
  const pending = proposals.some(item => item.status === 'pending');
  const terminal = run.desiredState === 'cancelled' || ['cancelled', 'closed', 'completed'].includes(run.status);
  const reason = terminal ? 'This run has ended. Specialist insertion is unavailable.' : disabledReason
    ?? (pending ? 'Wait for the existing team proposal to be applied.' : team.filter(item => !item.coordinator && !('retiredAt' in item && item.retiredAt)).length >= 3
      ? 'Three specialists are assigned. The coordinator must retire a responsibility before another can be added.' : !available.length ? 'No enabled specialist roles are available.' : undefined);
  return <details className="specialist-insertion">
    <summary>Add a specialist</summary>
    <p id={`insertion-help-${run.id}`} className="muted">{reason ?? (run.status === 'paused' || run.desiredState === 'paused'
      ? 'Save a proposal now. It waits for Resume and a safe coordination point.'
      : 'Save a proposal for the next safe coordination point. The coordinator receives the request.')}</p>
    <p className="muted">Readiness waits until the coordinator reconciles and closes the assignment. Changed code requires renewed checks and review.</p>
    <form aria-label="Add a specialist" onSubmit={async event => {
      event.preventDefault(); if (reason || saving.current) return;
      const form = event.currentTarget; const data = new FormData(form);
      const value = { roleId: String(data.get('roleId')), input: String(data.get('input')), dependsOn: data.getAll('dependsOn').map(String), criteria: data.getAll('criteria').map(Number) };
      const content = JSON.stringify(value);
      if (request.current?.content !== content) request.current = { content, id: crypto.randomUUID() };
      saving.current = true;
      try {
        if (await action(`/runs/${run.id}/specialists`, { ...value, requestId: request.current.id }, 'Specialist request saved. See its current status below.')) { request.current = undefined; form.reset(); }
      } finally { saving.current = false; }
    }}>
      <fieldset disabled={!!reason} aria-describedby={`insertion-help-${run.id}`}>
        <label>Specialist role<select name="roleId" required>{available.map(role => <option key={role.id} value={role.id}>{role.name} · {role.model || 'Model unavailable'}</option>)}</select></label>
        <label>Specialist input<textarea name="input" rows={3} required maxLength={40000} /></label>
        <fieldset><legend>Dependencies (optional)</legend>{team.map(item => <label className="insertion-choice" key={item.name}><input type="checkbox" name="dependsOn" value={item.name} />{item.name}</label>)}</fieldset>
        <fieldset><legend>Accepted criteria (optional for coding)</legend>{run.taskSnapshot.criteria.map((criterion, index) => <label className="insertion-choice" key={index}><input type="checkbox" name="criteria" value={index} />{criterion}</label>)}</fieldset>
        <button className="primary">Propose specialist</button>
      </fieldset>
    </form>
  </details>;
}
