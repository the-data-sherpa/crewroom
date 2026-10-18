import { attemptModel } from '../server/runtime/types';
import { useMemo, useState } from 'react';
import { Maximize2, Minimize2 } from 'lucide-react';
import type { Message, Run, Workspace } from '../shared/types';
import type { Assignment, Attempt, PeerMessage, TeamProposal } from '../server/runtime/types';
import type { DeliveryNotice } from '../shared/delivery';
import { runLabel } from '../shared/types';
import type { Action } from './main';
import { useLiveStatus, useRunEvents, useTopic, type LiveEvent } from './live';
import { LiveTerminal } from './terminal';
import { Markdown } from './markdown';
import type { RunHistory } from '../server/run-history';
import { UsagePanel, UsageNumbers } from './usage';
import { ticketHref } from './navigation';
import { runCauses, runNextAction } from '../shared/run-history';
import { steeringClosedReason, steeringTransitions } from '../shared/steering';
import { SpecialistInsertion } from './insertion';

export interface Command { attemptId: string; toolCallId: string; commandId: string; state: 'running' | 'exited' | 'cancelled'; exitCode?: number }
export interface Observation { history?: RunHistory; deliveryNotices?: DeliveryNotice[]; teamProposals?: TeamProposal[]; assignments: Assignment[]; attempts: Attempt[]; messages: PeerMessage[]; observedAttemptIds?: string[]; degradedAttemptIds?: string[]; commands?: Command[]; error?: string }

const deliveryLabels = { pending: 'Pending storage', stored: 'Stored in mailbox', accepted: 'Accepted for delivery', dispatched: 'Dispatching · Receipt unconfirmed', received: 'Received', uncertain: 'Storage uncertain' };
const steeringLabels = { pending: 'Stored · Not delivered', dispatched: 'Dispatched · Receipt unconfirmed', received: 'Received by coordinator · Not yet applied', applied: 'Applied to the plan', rejected: 'Rejected by coordinator', not_delivered: 'Not delivered' };
const activityLabels: Record<string, string> = { admitted: 'Queued · No process', starting: 'Starting', idle: 'Waiting for input', executing: 'Executing', failed: 'Failed', exited: 'Stopped', attention_required: 'Needs attention' };
const cycleLimit = (assignment: Assignment) => 2 + (assignment.repairAllowances?.length ?? 0);

export function eventText(event: LiveEvent['event']) {
  switch (event.type) {
    case 'host_ready': return 'Process and model verified';
    case 'agent_start': return 'Responding';
    case 'message_update': return event.phase?.startsWith('toolcall') ? 'Preparing a tool call' : 'Generating a response';
    case 'tool_started': return `Command started: ${event.commandId}`;
    case 'tool_exited': return `Command exited: ${event.result?.exitCode ?? 'result unavailable'}`;
    case 'prompt_result': return event.status === 'error' ? `Turn failed: ${event.error?.message ?? 'No error detail available'}` : `Turn ${event.status}`;
    case 'model_policy_rejected': return 'Blocked an unapproved model or reasoning setting';
    case 'process_exit': return `Runner exited: ${event.code != null ? `code ${event.code}` : 'exit code unknown'}${event.signal ? ` · signal ${event.signal}` : ''}`;
    case 'ownership_unresolved': return `Ownership unresolved: ${event.reason}`;
    default: return undefined;
  }
}
const isReply = (item: LiveEvent) => item.event.type === 'message_end' && item.event.message?.role === 'assistant';
const keepEvent = (item: LiveEvent) => isReply(item) || !!eventText(item.event);

export function useRunObservation(runId: string) {
  const { data, error } = useTopic<Observation>(`run:${runId}`);
  const events = useRunEvents(runId, keepEvent);
  return { observation: data, events, error };
}

function attemptStatus(observation: Observation, assignment: Assignment, attempt: Attempt) {
  if (assignment.retiredAt) return { label: 'Retired', tone: 'idle' };
  if (observation.degradedAttemptIds?.includes(attempt.id)) return { label: 'Observation degraded · Process live', tone: 'degraded' };
  if (!observation.observedAttemptIds?.includes(attempt.id) && !['admitted', 'exited'].includes(attempt.state)) return { label: 'Ownership unconfirmed', tone: 'attention' };
  const label = activityLabels[attempt.state] ?? attempt.state;
  return { label, tone: attempt.state === 'executing' ? 'executing' : ['failed', 'attention_required'].includes(attempt.state) ? 'attention' : 'idle' };
}

function RepairAllowance({ run, assignment, action, busy }: { run: Run; assignment: Assignment; action: Action; busy: boolean }) {
  const [open, setOpen] = useState(false);
  if (!open) return <button onClick={() => setOpen(true)} disabled={busy}>Allow another repair cycle…</button>;
  return <form className="allowance" onSubmit={async event => {
    event.preventDefault(); const reason = new FormData(event.currentTarget).get('reason');
    if (await action(`/runs/${run.id}/assignments/${assignment.id}/repair-allowance`, { requestId: crypto.randomUUID(), reason }, `One more automatic cycle allowed for ${assignment.name}.`)) setOpen(false);
  }}><label>Why allow another automatic cycle for {assignment.name}?<textarea name="reason" rows={2} required maxLength={2000} /></label>
    <div className="form-actions"><button className="primary" disabled={busy}>Allow one more cycle</button><button type="button" onClick={() => setOpen(false)}>Keep blocked</button></div>
  </form>;
}

function RunControls({ run, observation, action, busy, disabled, runnerAvailable }: { run: Run; observation?: Observation; action: Action; busy: boolean; disabled: boolean; runnerAvailable: boolean }) {
  const [confirmCancel, setConfirmCancel] = useState(false);
  const assignments = observation?.assignments ?? [];
  const limit = run.activeLimitMs ?? 3600000; const used = run.activeMs ?? 0;
  const canPause = ['running', 'awaiting_runner', 'pausing', 'attention_required'].includes(run.status) && assignments.length > 0;
  const canResume = run.desiredState !== 'cancelled' && ['paused', 'attention_required'].includes(run.status) && (assignments.length > 0 || (run.purpose ?? 'coding') === 'coding' && (!!run.schedule || run.slotRelease?.reason === 'migration'));
  const resumePending = !!run.resumeRequest;
  return <div className="run-controls">
    <span className="muted">Active time: {Math.floor(used / 60000)}m {Math.floor(used % 60000 / 1000)}s / {Math.round(limit / 60000)}m</span>
    {canPause && <button disabled={disabled || !runnerAvailable || run.status === 'pausing'} onClick={() => action(`/runs/${run.id}/pause`, {}, 'Execution paused.')}>{run.status === 'pausing' ? run.desiredState === 'cancelled' ? 'Cancelling…' : 'Pausing…' : 'Pause run'}</button>}
    {canResume && <button className="primary" disabled={disabled || !runnerAvailable || used >= limit || resumePending} onClick={() => action(`/runs/${run.id}/resume`, {}, 'Resume submitted. Check the run state for execution or queued status.')}>{run.resumeRequest?.state === 'queued' ? 'Resume queued' : run.resumeRequest?.state === 'starting' ? 'Resuming…' : 'Resume run'}</button>}
    {(run.purpose === 'explore' || run.purpose === 'grill') && run.status !== 'closed' && <button className="danger" disabled={disabled || run.status === 'pausing'} onClick={() => action(`/runs/${run.id}/close`, {}, 'Refinement closed. Coordinator stopped; transcript and proposals retained.')}>Close refinement</button>}
    {run.purpose !== 'explore' && run.purpose !== 'grill' && run.status !== 'cancelled' && run.status !== 'completed' && !confirmCancel && <button className="danger" disabled={disabled} onClick={() => setConfirmCancel(true)}>Cancel run</button>}
    {confirmCancel && <span className="confirm" role="group" aria-label="Confirm cancellation">
      <span>Cancel this run? Agents stop; changes, evidence and any PR are kept.</span>
      <button className="danger solid" disabled={disabled || busy} onClick={async () => { await action(`/runs/${run.id}/cancel`, {}, 'Cancellation requested. Work and history retained.'); setConfirmCancel(false); }}>Confirm cancel</button>
      <button onClick={() => setConfirmCancel(false)}>Keep run</button>
    </span>}
  </div>;
}

function Tile({ assignment, attempt, observation, events, focused, onFocus, run, action, busy, readOnly, currentRevision }: {
  assignment: Assignment; attempt: Attempt; observation: Observation; events: LiveEvent[]; focused: boolean; onFocus: () => void; run: Run; action: Action; busy: boolean; readOnly: boolean; currentRevision?: number;
}) {
  const status = attemptStatus(observation, assignment, attempt);
  const detail = observation.history?.attempts.find(item => item.attemptId === attempt.id);
  const usage = observation.history?.usage.rows.find(item => item.attemptId === attempt.id);
  const activity = events.filter(item => item.attemptId === attempt.id && eventText(item.event)).slice(-12);
  const exhausted = (assignment.automaticCycles ?? 0) >= cycleLimit(assignment);
  const model = attemptModel(assignment, attempt);
  const sameModel = model.requested.model === model.resolved.selector && model.requested.effort === model.resolved.effort;
  return <article className={`agent tile tone-${status.tone}`}>
    <header className="tile-head">
      <div className="tile-title">
        <h3>{assignment.name}</h3>
        <span className="tile-meta">{assignment.coordinator ? 'Coordinator' : assignment.role.name} · Role revision {assignment.role.revision ?? 1} · {model.resolved.selector} · {model.resolved.effort} · Attempt {attempt.generation}</span>
      </div>
      <span className={`agent-status ${status.tone}`}>{status.label}</span>
      <button className="icon" onClick={onFocus} aria-label={focused ? 'Show all agents' : `Enlarge ${assignment.name}`} aria-pressed={focused}>{focused ? <Minimize2 size={15} /> : <Maximize2 size={15} />}</button>
    </header>
    <LiveTerminal topic={`terminal:${attempt.id}`} label={assignment.name} />
    <footer className="tile-foot">
      <p className="agent-progress">{activity.length ? eventText(activity.at(-1)!.event) : 'No activity recorded'}</p>
      <span className="mono">{(attempt.runnerPid ?? attempt.ompPid) ? `${attempt.runnerKind ?? 'omp'} ${attempt.runnerPid ?? attempt.ompPid}` : 'no process'}{attempt.pane ? ` · tmux ${attempt.pane}` : ''}</span>
    </footer>
    {!!detail?.causes.length && <div className="callout compact" aria-label="Recorded attempt cause">{detail.causes.map(cause => <p key={cause}>{cause}</p>)}<p>Next: {detail.nextAction}</p></div>}
    {usage && <details><summary>Attempt usage</summary><UsageNumbers usage={usage} /><p className="muted">Source: {observation.history?.usage.source}.</p>{usage.unavailableReasons.map(reason => <p key={reason}>{reason}</p>)}</details>}
    <a className="attempt-link" href={ticketHref({ taskId: run.taskId, runId: run.id, attemptId: attempt.id })}>View {assignment.name} attempt {attempt.generation}</a>
    {exhausted && !assignment.retiredAt && <div className="callout compact">Automatic repair cycles used: {assignment.automaticCycles} / {cycleLimit(assignment)}. Further automatic repair is blocked until you allow it.
      {!readOnly && run.status !== 'cancelled' && <RepairAllowance run={run} assignment={assignment} action={action} busy={busy} />}</div>}
    <details><summary>Assignment definition</summary><dl>
      <dt>Role</dt><dd>{assignment.role.name} · revision {assignment.role.revision ?? 1}</dd>
      <dt>Resolved model / effort</dt><dd>{model.resolved.selector} · {model.resolved.effort}</dd>
      <dt>Role model selector / effort</dt><dd>{assignment.role.model || 'Configured default'} · {assignment.role.effort}</dd>
      <dt>Admitted instructions</dt><dd>{assignment.role.instructions}</dd>
    </dl></details>
    {currentRevision !== undefined && currentRevision !== (assignment.role.revision ?? 1) && <p className="callout compact">Current definition differs: revision {currentRevision}. This assignment retains revision {assignment.role.revision ?? 1}.</p>}
    <details><summary>Brief</summary>
      {!assignment.brief ? <p className="muted">No structured brief: admitted before structured briefs.</p> : <>
        {!assignment.brief.criteria.length ? <p className="muted">No accepted criteria assigned.</p> : <ul className="brief">{assignment.brief.criteria.map(item => <li key={item.index}>
          <strong>Criterion {item.index + 1}: {item.criterion}</strong><p>{item.obligation}</p>{!!item.checkIds.length && <p className="muted">Checks: {item.checkIds.join(', ')}</p>}</li>)}</ul>}
        <p className="muted">{assignment.brief.dependsOn.length ? `Depends on: ${assignment.brief.dependsOn.join(', ')}` : 'No dependencies on other assignments.'}</p>
      </>}
    </details>
    <details><summary>Process and recovery details</summary><dl>
      {!sameModel && <><dt>Requested model / effort</dt><dd>{model.requested.model} · {model.requested.effort}</dd></>}
      <dt>{attempt.runnerKind === 'claude-code' ? 'Claude Code' : attempt.runnerKind === 'codex' ? 'Codex' : 'OMP'} process</dt><dd>{attempt.runnerPid ?? attempt.ompPid ?? 'Not started'}</dd><dt>Host process</dt><dd>{attempt.hostPid ?? 'Not started'}</dd>
      {attempt.observedModel && <><dt>Observed model</dt><dd>{attempt.observedModel}</dd></>}
      <dt>Session</dt><dd>{attempt.sessionId ?? 'Not started'}{attempt.recoveryMode && ` · ${attempt.recoveryMode === 'session' ? 'saved session restored' : 'fresh session from retained inputs'}`}</dd>
      <dt>Ownership</dt><dd>{attempt.ownership ?? 'Unclaimed'}{attempt.ownershipReason && ` · ${attempt.ownershipReason}`}</dd>
      <dt>Automatic recovery cycles</dt><dd>{assignment.automaticCycles ?? 0} / {cycleLimit(assignment)}{assignment.repairAllowances?.map(item => <span key={item.id} className="block">Allowed {new Date(item.grantedAt).toLocaleString()}: {item.reason}</span>)}</dd>
    </dl><ol className="event-list">{activity.map(item => <li key={item.cursor}>{eventText(item.event)}</li>)}</ol></details>
  </article>;
}

export type RunObservation = ReturnType<typeof useRunObservation>;
export function AgentWall({ run, workspace, view: { observation, events, error }, action, busy, readOnly = false }: { run: Run; workspace: Workspace; view: RunObservation; action: Action; busy: boolean; readOnly?: boolean }) {
  const connected = useLiveStatus() === 'open';
  const [focus, setFocus] = useState<string>();
  const [roleId, setRoleId] = useState(workspace.roles.find(role => role.id === 'coordinator' && role.enabled && role.model)?.id ?? workspace.roles.find(role => role.enabled && role.model)?.id ?? workspace.roles.find(role => role.id === 'coordinator' && role.enabled)?.id ?? workspace.roles.find(role => role.enabled)?.id ?? '');
  const disabled = busy || !connected;
  const unavailable = !workspace.connections.runner;
  const assignments = observation?.assignments ?? [];
  const active = assignments.filter(item => !item.retiredAt);
  const proposals = observation?.teamProposals?.filter(item => item.status === 'pending') ?? [];
  const proposed = proposals.flatMap(proposal => proposal.specialists.map(specialist => ({ proposal, specialist })));
  const rejected = observation?.teamProposals?.filter(item => item.status === 'rejected') ?? [];
  const commands = observation?.commands ?? [];
  const running = commands.filter(item => item.state === 'running');
  const [openCheck, setOpenCheck] = useState<string>();
  const shownChecks = [...running, ...commands.filter(item => item.toolCallId === openCheck && item.state !== 'running')];
  const executing = active.filter(item => observation?.attempts.find(attempt => attempt.id === item.currentAttemptId)?.state === 'executing').length;
  const visible = focus ? active.filter(item => item.id === focus) : active;
  return <section className="wall" aria-labelledby={`wall-${run.id}`}>
    <div className="wall-head">
      <div><h2 id={`wall-${run.id}`}>Agent supervision</h2>
        <p className="muted">{observation ? `${active.length} ${active.length === 1 ? 'agent' : 'agents'} · ${executing} executing${proposed.length ? ` · ${proposed.length} proposed` : ''}${running.length ? ` · ${running.length} ${running.length === 1 ? 'check' : 'checks'} running` : ''}` : 'Loading saved activity…'}
          {readOnly ? ' · Read-only observation. This view does not send instructions or acknowledge mail.' : ' · Read-only terminals. Steer through the coordinator.'}</p></div>
      <span className="state-pill">{runLabel(run)}</span>
    </div>
    {!connected && <p role="alert" className="error">Observation disconnected. Retained activity may be out of date. Reconnecting…</p>}
    {error && <p role="alert" className="error">Observation unavailable: {error}. This does not establish an agent failure.</p>}
    {observation?.error && <p className="callout">Last controller error: {observation.error}. Saved in this server session; inspect the current run and process state to see whether it is resolved.</p>}
    {!!runCauses(run, workspace.queue.plans.find(plan => plan.id === run.id)?.holdReason).length && <div className="callout" aria-label="Run cause">{runCauses(run, workspace.queue.plans.find(plan => plan.id === run.id)?.holdReason).map(cause => <p key={cause}>{cause}</p>)}<p>Next: {runNextAction(run)}</p></div>}
    {observation?.history && <UsagePanel usage={observation.history.usage} />}
    {!readOnly && observation && !['explore', 'grill'].includes(run.purpose ?? 'coding') && <SpecialistInsertion key={run.id} run={run} roles={workspace.roles} assignments={assignments} proposals={observation.teamProposals ?? []} action={action}
      disabledReason={unavailable ? 'Runner integration is unavailable; specialist models cannot be validated.' : !connected ? 'Reconnect to propose a specialist.' : busy ? 'Saving an action. Please wait.' : undefined} />}
    {observation?.teamProposals?.some(proposal => proposal.origin === 'operator' && proposal.status === 'applied' && proposal.assignmentIds?.some(id => active.some(item => item.id === id))) && <p className="callout">Readiness waits for the coordinator to reconcile and close the inserted assignment. Evidence for unchanged code remains current.</p>}
    {run.pauseReason === 'budget' && <p className="callout">Active-time limit reached. Extend the limit before resuming.</p>}
    {!run.readinessHold && run.pauseReason === 'review' && <p className="callout">Paused at the human review gate. Select Resume to continue retained work; current validation status is shown in Change &amp; evidence.</p>}
    {run.readinessHold?.state === 'held' && <p className="callout">Readiness is no longer confirmed. This run holds the coding slot; only an explicit Resume starts agents to reconcile it.</p>}
    {run.readinessHold?.state === 'pending' && <p className="callout">Readiness is no longer confirmed. This run remains paused until prior process ownership and the coding slot are available; only an explicit Resume starts agents.</p>}
    {run.resumeRequest?.state === 'queued' && <p className="callout">Resume is queued. The run remains paused until a safe coding slot is available; it does not preempt another run and waits for prior ownership to be confirmed.</p>}
    {!run.readinessHold && run.pauseReason === 'validation' && <p className="callout">Validation no longer supports review readiness. Only an explicit Resume starts agents to reconcile the result.</p>}
    {run.position !== undefined && <p className="muted">{workspace.queue.plans.find(plan => plan.id === run.id)?.holdReason ?? 'Eligible · scheduling'}</p>}
    {!readOnly && <RunControls run={run} observation={observation} action={action} busy={busy} disabled={disabled} runnerAvailable={!unavailable} />}
    {!readOnly && <a className="observe-link" href={`/?observe=${run.id}`} target="_blank" rel="noreferrer">Open observation view</a>}
    {run.status === 'cancelled' && <p className="muted">Execution ended. Inputs, changes and history are retained.</p>}
    {!readOnly && run.pauseReason === 'budget' && <form className="budget-form" onSubmit={event => { event.preventDefault(); const value = new FormData(event.currentTarget).get('minutes'); void action(`/runs/${run.id}/budget`, { activeLimitMs: Number(value) * 60000 }, 'Active-time limit updated.'); }}><label>Total active-time limit (minutes)<input name="minutes" type="number" min={1} step={1} defaultValue={Math.ceil((run.activeLimitMs ?? 3600000) / 60000) + 30} required /></label><button disabled={disabled}>Set limit</button></form>}
    {!!observation?.deliveryNotices?.length && <details><summary>Controller delivery notices</summary><ul>{observation.deliveryNotices.map(notice => <li key={notice.id}>{notice.payload.kind} · {notice.disposition} · {new Date(notice.createdAt).toLocaleString()}</li>)}</ul></details>}
    {observation && !assignments.length && <p className="muted">No agents have started. {unavailable ? 'Runner integration is unavailable.' : run.status === 'cancelled' ? 'Execution ended; no agents can start on this run.' : run.readinessHold ? 'Readiness is not confirmed; only an explicit Resume starts this retained run.' : run.slotRelease?.reason === 'review' ? 'Paused at the human review gate. Select Resume to continue; this released run does not start automatically.' : run.status === 'paused' || run.status === 'attention_required' || run.status === 'pausing' ? 'Only an explicit Resume can start this retained run.' : run.purpose === 'coding' ? run.position !== undefined && run.launch ? 'Queued coding starts automatically when eligible.' : 'Save launch details to start automatically, or start manually when the coding slot is free.' : 'Start the coordinator with a configured role.'}</p>}
    {!readOnly && observation && !assignments.length && !unavailable && run.status === 'awaiting_runner' && (run.purpose === 'explore' || run.purpose === 'grill' ? <button className="primary" disabled={disabled} onClick={() => action(`/runs/${run.id}/start`, {}, 'Refinement coordinator started.')}>Start refinement coordinator</button> : <form className="start-agent" onSubmit={event => { event.preventDefault(); void action(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId, coordinator: true, input: JSON.stringify({ brief: run.taskSnapshot.brief, criteria: run.taskSnapshot.criteria }) }] }, 'Coordinator started.'); }}><label>Coordinator role<select value={roleId} onChange={event => setRoleId(event.target.value)}>{workspace.roles.filter(role => role.enabled).map(role => <option key={role.id} value={role.id}>{role.name} · {role.model}</option>)}</select></label><button className="primary" disabled={disabled || !roleId}>Start coordinator</button>{run.purpose !== 'research' && <small>Manual coding start requires a free coding slot.</small>}</form>)}
    {observation && <div className={`agent-grid${focus ? ' focused' : ''}`}>
      {visible.map(assignment => {
        const attempt = observation.attempts.find(item => item.id === assignment.currentAttemptId)!;
        const currentRole = workspace.roles.find(role => role.id === assignment.role.id);
        return <Tile key={assignment.id} assignment={assignment} attempt={attempt} observation={observation} events={events} focused={focus === assignment.id}
          onFocus={() => setFocus(focus === assignment.id ? undefined : assignment.id)} run={run} action={action} busy={busy} readOnly={readOnly} currentRevision={currentRole ? currentRole.revision ?? 1 : undefined} />;
      })}
      {!focus && proposed.map(({ proposal, specialist }) => <article key={`${proposal.id}:${specialist.name}`} className="tile proposed">
        <header className="tile-head"><div className="tile-title"><h3>{specialist.name}</h3><span className="tile-meta">{specialist.role.name} · {specialist.model.resolved.selector} · {specialist.model.resolved.effort}</span></div><span className="agent-status proposed">Proposed · not started</span></header>
        <p>{proposal.origin === 'operator' ? 'You proposed this specialist.' : 'The coordinator proposed this specialist.'} Crewroom starts it at the next safe stopping point.{run.status === 'paused' || run.desiredState === 'paused' ? ' Waiting for Resume.' : ''}</p><p className="muted">No process exists yet, so there is no terminal to show.</p>
        <p>{specialist.input}</p>{!!specialist.brief?.dependsOn.length && <p>Depends on: {specialist.brief.dependsOn.join(', ')}</p>}
      </article>)}
      {!focus && rejected.map(proposal => <article key={proposal.id} className="tile proposed" aria-label="Rejected team proposal">
        <header className="tile-head"><div className="tile-title"><h3>Team proposal</h3><span className="tile-meta">{proposal.specialists.map(item => `${item.name} · ${item.role.name}`).join(', ')}</span></div><span className="agent-status attention">Rejected · not admitted</span></header>
        <p>{proposal.reason ?? 'This proposal was not applied.'}</p>
      </article>)}
    </div>}
    {!focus && shownChecks.map(command => <article key={command.toolCallId} className="tile check">
      <header className="tile-head"><div className="tile-title"><h3>Check · {command.commandId}</h3><span className="tile-meta">{assignments.find(item => item.currentAttemptId === command.attemptId || observation?.attempts.find(attempt => attempt.id === command.attemptId)?.assignmentId === item.id)?.name ?? 'Unknown agent'} · container, network off</span></div>
        <span className={`agent-status ${command.state === 'running' ? 'executing' : command.exitCode === 0 ? 'idle' : 'attention'}`}>{command.state === 'running' ? 'Running' : command.state === 'cancelled' ? 'Cancelled' : `Exited ${command.exitCode ?? '(unknown)'}`}</span>
        {command.state !== 'running' && <button className="icon" aria-label="Close check output" onClick={() => setOpenCheck(undefined)}><Minimize2 size={15} /></button>}</header>
      <LiveTerminal topic={`check:${command.attemptId}:${command.toolCallId}`} label={`Check ${command.commandId}`} convertEol />
    </article>)}
    {!focus && commands.some(item => item.state !== 'running') && <div className="history-strip"><span className="muted">Finished commands</span>
      {commands.filter(item => item.state !== 'running').slice(-8).map(item => <button key={item.toolCallId} className="chip" aria-pressed={openCheck === item.toolCallId} onClick={() => setOpenCheck(openCheck === item.toolCallId ? undefined : item.toolCallId)}>{item.commandId} · {item.state === 'cancelled' ? 'cancelled' : `exit ${item.exitCode ?? '?'}`}</button>)}</div>}
  </section>;
}

function RecordedTime({ at }: { at?: string }) {
  return at ? <time dateTime={at} title={at}>{new Date(at).toLocaleString([], { dateStyle: 'medium', timeStyle: 'medium' })}</time> : <>Time unknown</>;
}

function SteeringFeedback({ message }: { message: Message }) {
  const labels = { pending: 'Delivery deferred', dispatched: 'Dispatched', received: 'Receipt confirmed', applied: 'Applied to plan', rejected: 'Rejected', not_delivered: 'Not delivered' };
  return <div className="steering-feedback">
    <p className={`delivery ${message.delivery}`}>{steeringLabels[message.delivery]}</p>
    {message.delivery === 'applied' && <p className="muted">Reconciled into the plan or an explicit response. This does not confirm completed work.</p>}
    {message.dispositionReason && <p className="steering-reason">{message.dispositionReason}</p>}
    <dl className="steering-times" aria-label="Recorded steering times">
      <div><dt>Stored</dt><dd><RecordedTime at={message.createdAt} /></dd></div>
      {steeringTransitions(message).map((transition, index) => <div key={index}>
        <dt>{labels[transition.state]}</dt><dd><RecordedTime at={transition.at} />
          {transition.state === 'pending' && transition.reason && <span>{transition.reason}</span>}
        </dd>
      </div>)}
    </dl>
  </div>;
}

export function CoordinatorPanel({ run, workspace, view: { observation, events }, action, busy, readOnly = false }: { run: Run; workspace: Workspace; view: RunObservation; action: Action; busy: boolean; readOnly?: boolean }) {
  const connected = useLiveStatus() === 'open';
  const assignments = observation?.assignments ?? [];
  const coordinator = assignments.find(item => item.coordinator);
  const coordinatorAttempts = useMemo(() => new Set(observation?.attempts.filter(attempt => attempt.assignmentId === coordinator?.id).map(attempt => attempt.id)), [observation, coordinator?.id]);
  const replies = events.filter(item => coordinatorAttempts.has(item.attemptId) && isReply(item)).flatMap(item => {
    const text = item.event.message?.content?.filter((part: any) => part.type === 'text').map((part: any) => part.text).join('\n');
    return text ? [{ key: `${item.attemptId}:${item.sequence}`, text, timestamp: item.event.message?.timestamp ?? 0, message: undefined }] : [];
  });
  const steering = workspace.messages.filter(message => message.runId === run.id);
  const conversation = [...steering.map(message => ({ key: message.id, text: message.body, timestamp: Date.parse(message.createdAt), message })), ...replies].sort((a, b) => a.timestamp - b.timestamp);
  const time = (value: number) => value > 1e11 ? new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '';
  const closedReason = steeringClosedReason(run);
  const disabledReason = closedReason ? `${closedReason} New steering is unavailable.`
    : !connected ? 'Connection unavailable. Reconnect to store steering.' : busy ? 'Saving an action. Please wait.' : undefined;
  const composerHelp = disabledReason ?? (run.status === 'paused' || run.status === 'pausing' || run.desiredState === 'paused'
    ? 'Saved now; delivered after Resume.' : 'Stored first. Receipt and application show on the message.');
  return <div className="coordinator-panel">
    <section aria-labelledby={`conversation-${run.id}`} className="conversation">
      <h2 id={`conversation-${run.id}`} className="visually-hidden">Coordinator conversation</h2>
      {!!observation?.deliveryNotices?.some(notice => notice.payload.kind === 'operator_specialist_proposed') && <ul className="messages">{observation.deliveryNotices.filter(notice => notice.payload.kind === 'operator_specialist_proposed').map(notice => {
        const proposal = observation.teamProposals?.find(item => item.id === notice.payload.subjectId);
        return <li key={notice.id} className="operator-message"><strong>Specialist insertion</strong><p>{proposal?.specialists.map(item => `${item.role.name}: ${item.input}`).join('\n')}</p>
          <small>{notice.disposition === 'received' ? 'Coordinator receipt confirmed' : notice.disposition === 'dispatched' ? 'Dispatched · Receipt unconfirmed' : 'Stored · Coordinator receipt unconfirmed'} · <RecordedTime at={notice.createdAt} /></small>
          <p>{proposal?.status === 'rejected' ? proposal.reason : proposal?.status === 'applied' ? proposal.assignmentIds?.every(id => assignments.find(item => item.id === id)?.retiredAt)
            ? 'The coordinator closed the inserted assignment. Current readiness is shown in Change & evidence.' : 'Assignment admitted. Readiness waits until the coordinator closes it.' : 'Proposed · Not started'}</p></li>;
      })}</ul>}
      {!conversation.length && <p className="muted">The coordinator’s replies and your steering will appear here.</p>}
      <ul className="messages">{conversation.map(entry => <li key={entry.key} className={entry.message ? 'operator-message' : 'coordinator-message'}>
        <div className="message-head"><strong>{entry.message ? entry.message.kind === 'requirements_adoption' ? 'Requirement decision' : 'You' : 'Coordinator'}</strong><time>{time(entry.timestamp)}</time></div>
        {entry.message ? <p>{entry.text}</p> : <Markdown text={entry.text} />}
        {entry.message ? <SteeringFeedback message={entry.message} /> : <small className="muted">Coordinator reply · Steering application requires a recorded disposition.</small>}
      </li>)}</ul>
    </section>
    {!readOnly && <form className="composer" onSubmit={async event => {
      event.preventDefault(); const form = event.currentTarget; const body = new FormData(form).get('message');
      if (disabledReason) return;
      if (await action(`/runs/${run.id}/steering`, { requestId: crypto.randomUUID(), body }, 'Steering stored.')) form.reset();
    }}><p id={`composer-help-${run.id}`} className="muted">{composerHelp}</p><fieldset disabled={!!disabledReason}>
      <label>Message to coordinator<textarea name="message" rows={3} required maxLength={40000} aria-describedby={`composer-help-${run.id}`} onKeyDown={event => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) event.currentTarget.form?.requestSubmit(); }} /></label>
      <div className="composer-actions"><span className="muted">Ctrl/⌘ + Enter to send</span><button className="primary">Send to coordinator</button></div>
    </fieldset></form>}
    <section className="peer" aria-labelledby={`peer-${run.id}`}>
      <h3 id={`peer-${run.id}`}>Peer messages</h3><p className="muted">Mailbox receipt confirms delivery, not completed work.</p>
      {!observation?.messages.length ? <p className="muted">No peer messages recorded.</p> : <details><summary>{observation.messages.length} {observation.messages.length === 1 ? 'message' : 'messages'}</summary>
        <ul className="messages">{observation.messages.map(message => <li key={message.id}><strong>{assignments.find(item => item.id === message.from)?.name ?? 'Unknown sender'} → {assignments.find(item => item.id === message.to)?.name ?? 'Unknown recipient'}</strong><p>{message.body}</p><small>{deliveryLabels[message.disposition]}</small></li>)}</ul></details>}
    </section>
  </div>;
}
