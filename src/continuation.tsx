import { useRef } from 'react';
import type { Run, Task, Workspace } from '../shared/types';
import type { Action } from './main';
import { ticketHref } from './navigation';

export function RunContinuation({ run, task, workspace, roleId, action, busy, unsaved }: {
  run: Run; task: Task; workspace: Workspace; roleId: string; action: Action; busy: boolean; unsaved: boolean;
}) {
  const request = useRef<{ payload: string; id: string } | undefined>(undefined);
  const retained = workspace.continuations?.[run.id];
  const reason = retained?.reason ?? (unsaved ? 'Save or discard brief changes before continuing.'
    : task.needsReacceptance ? 'Accept the changed source requirements before continuing.' : undefined);
  return <section className="callout continuation" aria-label="Run continuation">
    {run.continuedFrom && <p>Continued from <a href={ticketHref({ taskId: task.id, runId: run.continuedFrom.runId })}>cancelled predecessor</a>. Earlier evidence stays in that run; this run requires fresh checks and review.</p>}
    {run.continuedBy ? <p>Continued in <a href={ticketHref({ taskId: task.id, runId: run.continuedBy })}>linked successor</a>. This run remains cancelled.</p>
      : run.desiredState === 'cancelled' && (run.purpose ?? 'coding') === 'coding' && <>
        <p>{retained?.branch ? <>Retained branch <code>{retained.branch}</code> at <code>{retained.head?.slice(0, 12) ?? 'unknown head'}</code>.</> : 'No owned branch was recorded for this run.'}
          {retained?.prUrl && <> <a href={retained.prUrl} target="_blank" rel="noreferrer">Existing PR</a></>}</p>
        {reason ? <p>{reason}</p> : <p>Continue later queues a new linked run on this branch{retained?.prUrl ? ' and PR' : ''} using accepted brief revision {task.revision} and the selected coordinator role. The cancelled run stays closed; previous evidence is historical.</p>}
        {retained?.branch && <button className="primary" disabled={busy || !!reason || !roleId || !retained} onClick={() => {
          const payload = JSON.stringify({ runId: run.id, revision: task.revision, roleId });
          if (request.current?.payload !== payload) request.current = { payload, id: crypto.randomUUID() };
          void action(`/runs/${run.id}/continue-later`, { requestId: request.current.id, revision: task.revision, roleId }, 'Successor saved. Check its run state for progress.');
        }}>Continue later</button>}
      </>}
  </section>;
}
