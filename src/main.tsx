import { RunContinuation } from './continuation';
import { useEffect, useState, type FormEvent } from 'react';
import { createRoot } from 'react-dom/client';
import { ArrowUpRight, FolderGit2, Layers3, Plus } from 'lucide-react';
import type { Task, Workspace } from '../shared/types';
import { runLabel } from '../shared/types';
import { runOutcome } from '../shared/run-history';
import { navigateTicket, parseTicketLocation, useTicketLocation, type TicketLocation } from './navigation';
import { AttemptHistory } from './attempt-history';
import './style.css';
import { AgentWall, CoordinatorPanel, useRunObservation } from './wall';
import { ResultPanel } from './results';
import { RoleEditor } from './roles';
import { LearningPanel } from './learning';
import { RefinementPanel } from './refinement';
import { JiraSourcePanel, JiraUpdates } from './jira';
import { useLiveStatus, useTopic, live } from './live';

async function api(path: string, body?: unknown) {
  const response = await fetch(`/api${path}`, body === undefined ? {} : {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error ?? 'Unable to save. Try again.');
  return result;
}
export type Action = (path: string, body: unknown, success: string) => Promise<any>;
const data = (event: FormEvent<HTMLFormElement>) => { event.preventDefault(); return new FormData(event.currentTarget); };
type Draft = { brief: string; criteria: string };

/** Resolves on the next pushed workspace that differs from the one already shown, or after a short timeout. */
function freshWorkspace(timeout = 1000) {
  return new Promise<void>(resolve => {
    let initial: unknown; let first = true;
    const timer = setTimeout(() => { stop(); resolve(); }, timeout);
    const stop = live.subscribe('workspace', message => {
      if (message.kind !== 'state') return;
      if (first) { first = false; initial = message.data; return; }
      if (message.data !== initial) { clearTimeout(timer); queueMicrotask(() => stop()); resolve(); }
    });
  });
}
/** Waits briefly for a pushed workspace that satisfies a condition (after a mutation). */
function awaitWorkspace(check: (workspace: Workspace) => boolean, timeout = 3000) {
  return new Promise<void>(resolve => {
    const timer = setTimeout(() => { stop(); resolve(); }, timeout);
    const stop = live.subscribe('workspace', message => { if (message.kind === 'state' && check(message.data)) { clearTimeout(timer); queueMicrotask(() => stop()); resolve(); } });
  });
}

function ticketStatus(workspace: Workspace, task: Task) {
  const run = workspace.runs.filter(item => item.taskId === task.id).at(-1);
  if (!run) return { label: task.needsReacceptance ? 'Source changed · review brief' : 'Refining', tone: task.needsReacceptance ? 'attention' : 'idle' };
  const executing = workspace.activity.filter(item => item.runId === run.id && item.state === 'executing').length;
  const tone = run.status === 'attention_required' ? 'attention' : run.status === 'running' ? 'executing' : 'idle';
  return { label: `${runLabel(run)}${executing ? ` · ${executing} executing` : ''}`, tone };
}

function App() {
  const { data: workspace } = useTopic<Workspace>('workspace');
  const status = useLiveStatus();
  const route = useTicketLocation(); const selected = route.taskId;
  const [panel, setPanel] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const disconnected = status === 'closed';
  const observationId = new URLSearchParams(location.search).get('observe');
  const unsaved = Object.keys(drafts).length > 0;
  useEffect(() => {
    const prevent = (event: BeforeUnloadEvent) => { if (unsaved) event.preventDefault(); };
    addEventListener('beforeunload', prevent); return () => removeEventListener('beforeunload', prevent);
  }, [unsaved]);
  const action: Action = async (path, body, success) => {
    setBusy(true); setError(''); setNotice('Saving…');
    try { const pushed = freshWorkspace(); const result = await api(path, body); await pushed; setNotice(success); return result; }
    catch (error) { setError(error instanceof Error ? error.message : 'Unable to save. Try again.'); setNotice(''); return null; }
    finally { setBusy(false); }
  };
  const setDraft = (taskId: string, draft?: Draft) => setDrafts(current => { const next = { ...current }; if (draft) next[taskId] = draft; else delete next[taskId]; return next; });
  const task = workspace?.tasks.find(task => task.id === selected);
  if (observationId) return <ObservationPage runId={observationId} workspace={workspace} disconnected={disconnected} />;
  return <>
    <a className="skip" href="#workspace">Skip to workspace</a>
    <header className="topbar"><a className="brand" href="#"><Layers3 size={22} />Crewroom</a>
      <span className={`live-pill ${status}`}><i />{status === 'open' ? 'Live' : status === 'connecting' ? 'Connecting…' : 'Reconnecting…'}</span>
      <span className="local">Local workbench</span></header>
    <div className="layout">
      <aside><div className="rail-title"><span>WORK</span><span>{workspace?.tasks.length ?? 0}</span></div>
        <button className="primary wide" onClick={() => setPanel(panel === 'new' ? '' : 'new')}><Plus size={16} />New ticket</button>
        <nav aria-label="Tickets">{workspace?.tasks.map(item => { const state = ticketStatus(workspace, item); return <a key={item.id} href={`#${item.id}`} aria-current={selected === item.id ? 'page' : undefined} onClick={() => setPanel('')}>
          <small>{item.key} · {item.source}</small><strong>{item.title}</strong>
          <span className={`rail-status ${state.tone}`}><i />{state.label}{drafts[item.id] && ' · unsaved brief'}</span></a>; })}</nav>
        {workspace?.tasks.length === 0 && <p className="muted rail-empty">Create a ticket or import a Jira snapshot to begin.</p>}
        {workspace && <QueuePanel workspace={workspace} action={action} busy={busy || disconnected} />}
        <div className="rail-bottom"><button onClick={() => setPanel(panel === 'learning' ? '' : 'learning')}>Learning{workspace?.learning.some(item => item.status === 'pending') ? ` · ${workspace.learning.filter(item => item.status === 'pending').length} to decide` : ''}</button><button onClick={() => setPanel(panel === 'roles' ? '' : 'roles')}>Roles</button><button onClick={() => setPanel(panel === 'import' ? '' : 'import')}><ArrowUpRight size={16} />Import Jira snapshot</button><button onClick={() => setPanel(panel === 'repositories' ? '' : 'repositories')}><FolderGit2 size={16} />Repositories</button></div>
      </aside>
      <main id="workspace">
        <div role="status" aria-live="polite" className="notice">{notice}</div>
        {disconnected && <div role="alert" className="error">Connection lost. Retained activity may be out of date. Reconnecting…</div>}
        {error && <div role="alert" className="error">{error}</div>}
        {workspace && <JiraSourcePanel workspace={workspace} action={action} busy={busy || disconnected} />}
        {!workspace && !error && <p>Loading your workspace…</p>}
        {workspace && <section className="card setup" hidden={panel !== 'roles'}>
          <div className="section-title"><h2>User roles</h2><button onClick={() => setPanel('')}>Close</button></div>
          <RoleEditor roles={workspace.roles} action={action} busy={busy || disconnected} />
        </section>}
        {workspace && panel && panel !== 'roles' && <section className="card setup">
          <div className="section-title"><h2>{panel === 'new' ? 'New local ticket' : panel === 'import' ? 'Import a Jira snapshot' : panel === 'learning' ? 'Learning proposals' : 'Repository configuration'}</h2><button onClick={() => setPanel('')}>Close</button></div>
          {panel === 'new' && <form onSubmit={async event => {
            const values = data(event); const created = await action('/tasks', { title: values.get('title'), description: values.get('description') }, 'Ticket created.');
            if (created) { await awaitWorkspace(value => value.tasks.some(item => item.id === created.id)); location.hash = created.id; setPanel(''); }
          }}><fieldset disabled={busy}><label>Ticket title<input name="title" required maxLength={300} autoFocus /></label><label>Description<textarea name="description" rows={4} /></label><button className="primary">Create ticket</button></fieldset></form>}
          {panel === 'import' && <form onSubmit={async event => {
            const values = data(event);
            try { const tasks = await action('/import/jira', JSON.parse(String(values.get('json'))), 'Jira snapshot imported. Local refinements retained.');
              if (tasks) { await awaitWorkspace(value => value.tasks.some(item => item.id === tasks[0].id && item.sourceRevision === tasks[0].sourceRevision)); location.hash = tasks[0].id; setPanel(''); }
            } catch { setError('Enter a JSON object with siteUrl and issues.'); }
          }}><p className="muted">{workspace.jira ? 'Import a saved snapshot manually. Live refresh uses the configured source.' : 'Live Jira synchronization is unavailable. Import saved source fields; local briefs stay separate.'}</p><fieldset disabled={busy}><label>Jira JSON<textarea name="json" rows={8} required placeholder={'{"siteUrl":"https://example.atlassian.net","issues":[…]}'} /></label><button className="primary">Import snapshot</button></fieldset></form>}
          {panel === 'learning' && <LearningPanel workspace={workspace} action={action} busy={busy || disconnected} />}
          {panel === 'repositories' && <RepositoriesPanel repositories={workspace.repositories} action={action} busy={busy || disconnected} />}
        </section>}
        {workspace && !panel && (task ? <Ticket key={task.id} route={route} task={task} workspace={workspace} action={action} busy={busy || disconnected} draft={drafts[task.id]} setDraft={draft => setDraft(task.id, draft)} />
          : <section className="empty"><div className="empty-icon"><Layers3 size={32} /></div><h1>No ticket selected</h1><p>Choose a ticket, or create one to shape its brief.</p><button className="primary" onClick={() => setPanel('new')}>Create a ticket</button></section>)}
        <footer><span><i />{workspace?.jira ? workspace.jira.state === 'connected' ? 'Jira refreshed' : workspace.jira.state === 'unavailable' ? 'Jira unavailable' : 'Jira configured — not refreshed' : 'Jira synchronization unavailable'}</span><span><i />{workspace?.connections.runner ? 'Runner configured' : 'Runner unavailable'}</span><span>Changes saved on this machine</span></footer>
      </main>
    </div>
  </>;
}
function QueuePanel({ workspace, action, busy }: { workspace: Workspace; action: Action; busy: boolean }) {
  const plans = workspace.queue.plans;
  const reorder = async (index: number, direction: -1 | 1) => {
    const runIds = plans.map(plan => plan.id);
    [runIds[index], runIds[index + direction]] = [runIds[index + direction], runIds[index]];
    const saved = await action('/queue/order', { requestId: crypto.randomUUID(), expectedRevision: workspace.queue.revision, runIds }, 'Queue order saved.');
    if (!saved) { const stop = live.subscribe('workspace', () => {}, { restart: true }); queueMicrotask(stop); }
  };
  return <section className="queue-panel" aria-label="Queue"><div className="rail-title"><span>QUEUE</span><span>{plans.length}</span></div>
    {workspace.queue.slot && <p className="muted">Coding slot: <strong>{workspace.queue.slot.key}</strong> · {workspace.queue.slot.reason}</p>}
    {!plans.length && <p className="muted">No coding plans queued.</p>}
    <ol>{plans.map((plan, index) => {
      const repository = workspace.repositories.find(item => item.id === plan.taskSnapshot.repositoryId);
      return <li key={plan.id}><a href={`#${plan.taskId}`}><strong>{plan.taskSnapshot.key}</strong> · {plan.taskSnapshot.title}</a>
        <small>{repository?.name ?? 'Repository unavailable'} · {plan.launch?.team.find(item => item.coordinator)?.roleId ?? 'Coordinator not selected'}</small>
        <small>{plan.launch ? 'Launch details saved' : 'Launch details missing'} · Queued plan, not running</small>
        <small>Hold reason: {plan.holdReason ?? 'Eligible · scheduling'}</small>
        <div className="queue-controls"><button type="button" aria-label={`Move ${plan.taskSnapshot.key} up`} disabled={busy || index === 0} onClick={() => reorder(index, -1)}>Move up</button>
          <button type="button" aria-label={`Move ${plan.taskSnapshot.key} down`} disabled={busy || index === plans.length - 1} onClick={() => reorder(index, 1)}>Move down</button>
          <button type="button" className="danger" aria-label={`Remove ${plan.taskSnapshot.key} from queue`} disabled={busy} onClick={() => action(`/queue/${plan.id}/remove`, { requestId: crypto.randomUUID() }, 'Queued plan cancelled. History retained.')}>Remove</button></div></li>;
    })}</ol></section>;
}


function RepositoriesPanel({ repositories, action, busy }: { repositories: Workspace['repositories']; action: Action; busy: boolean }) {
  const [editing, setEditing] = useState<{ id: string; revision: number }>();
  const [rows, setRows] = useState<number[]>([]);
  const [nextRow, setNextRow] = useState(0);
  const selected = repositories.find(repo => repo.id === editing?.id);
  const edit = (repo: Workspace['repositories'][number]) => {
    setEditing({ id: repo.id, revision: repo.revision ?? 1 });
    setRows((repo.checks ?? []).map((_, index) => index)); setNextRow(repo.checks?.length ?? 0);
  };
  return <><ul className="repository-list">{repositories.map(repo => <li key={repo.id}><strong>{repo.name}{repo.retiredAt ? ' · Retired' : ''}</strong>
    <span className="mono">{repo.path}</span><span>Target: {repo.targetBranch} · Branch prefix: {repo.branchPrefix ?? 'codex/crewroom/'} · {repo.checks?.length ?? 0} checks · PR publication {repo.publicationAvailable ? 'available' : 'unavailable (no Bitbucket connection)'}</span>
    <span>Revision {repo.revision ?? 1}</span>
    {!repo.retiredAt && <div className="repository-actions"><button type="button" disabled={busy} aria-label={`Edit ${repo.name}`} onClick={() => edit(repo)}>Edit</button>
      <button type="button" disabled={busy} className="danger" aria-label={`Retire ${repo.name}`} onClick={async () => {
        if (!window.confirm(`Retire ${repo.name}? It will disappear from new selection. Historical runs remain readable; tickets selecting it must choose another repository before Run.`)) return;
        if (await action(`/repositories/${repo.id}/retire`, { revision: repo.revision ?? 1, requestId: crypto.randomUUID() }, 'Repository retired. Historical runs remain readable.')) {
          if (editing?.id === repo.id) setEditing(undefined);
        }
      }}>Retire</button></div>}
  </li>)}</ul><form key={editing ? `${editing.id}:${editing.revision}` : 'new'} onSubmit={async event => {
    const form = event.currentTarget; const values = data(event);
    const checks = rows.map(row => ({ id: String(values.get(`check-id-${row}`)), image: String(values.get(`check-image-${row}`)),
      command: String(values.get(`check-command-${row}`)), args: String(values.get(`check-args-${row}`)).split('\n').map(arg => arg.replace(/\r$/, '')).filter(arg => arg !== ''),
      timeoutMs: Number(values.get(`check-timeout-${row}`)), ...(values.get(`check-memory-${row}`) ? { memoryMiB: Number(values.get(`check-memory-${row}`)) } : {}), required: values.has(`check-required-${row}`) }));
    const branchPrefix = String(values.get('prefix')).trim();
    const configuration = { name: values.get('name'), path: values.get('path'), targetBranch: values.get('branch'), ...(branchPrefix ? { branchPrefix } : {}), checks };
    const saved = editing ? await action(`/repositories/${editing.id}`, { ...configuration, revision: editing.revision, requestId: crypto.randomUUID() }, 'Repository updated.')
      : await action('/repositories', configuration, 'Repository saved.');
    if (saved) { form.reset(); setRows([]); setEditing(undefined); }
  }}><fieldset disabled={busy}><h3>{editing ? `Edit repository · revision ${editing.revision}` : 'Add repository'}</h3>
    {editing && selected && (selected.revision ?? 1) !== editing.revision && <p className="callout">The saved configuration is now revision {selected.revision}. Your draft is retained. Reload only if you want to discard it.</p>}
    {editing && selected?.retiredAt && <p className="callout">This repository was retired. Copy any draft before closing the editor.</p>}
    <label>Repository name<input name="name" required maxLength={120} defaultValue={selected?.name} /></label>
    <label>Checkout path<input name="path" required placeholder="/absolute/path/to/repository" defaultValue={selected?.path} /></label>
    <label>Target branch<input name="branch" required defaultValue={selected?.targetBranch ?? 'main'} /></label>
    <label>Branch prefix<span className="hint">Optional; ends in /. Default: codex/crewroom/</span><input name="prefix" placeholder="codex/crewroom/" defaultValue={selected?.branchPrefix} /></label>
    <h3>Repository checks</h3><p className="muted">Checks run from pinned container images. Leave this empty if none are required.</p>
    {rows.map((row, index) => { const check = selected?.checks?.[row]; return <div className="check-row" key={row} role="group" aria-label={`Check ${index + 1}`}><div className="section-title"><h3>Check {index + 1}</h3><button type="button" onClick={() => setRows(current => current.filter(item => item !== row))}>Remove check {index + 1}</button></div>
      <label>Check ID<input name={`check-id-${row}`} required pattern="[a-z][a-z0-9-]*" maxLength={64} defaultValue={check?.id} /></label>
      <label>Image digest<span className="hint">sha256: followed by 64 lowercase hex characters</span><input name={`check-image-${row}`} required pattern="sha256:[a-f0-9]{64}" placeholder="sha256:…" defaultValue={check?.image} /></label>
      <label>Command<input name={`check-command-${row}`} required placeholder="/usr/bin/node" defaultValue={check?.command} /></label>
      <label>Arguments<span className="hint">One argument per line</span><textarea name={`check-args-${row}`} rows={3} defaultValue={check?.args.join('\n')} /></label>
      <div className="check-limits"><label>Timeout (ms)<input name={`check-timeout-${row}`} type="number" min={100} max={3600000} step={1} required defaultValue={check?.timeoutMs ?? 60000} /></label>
        <label>Memory (MiB)<span className="hint">Optional; 128–8192</span><input name={`check-memory-${row}`} type="number" min={128} max={8192} step={1} defaultValue={check?.memoryMiB} /></label></div>
      <label className="checkbox"><input name={`check-required-${row}`} type="checkbox" defaultChecked={check?.required ?? true} />Required</label>
    </div>; })}
    <div className="form-actions"><button type="button" onClick={() => { setRows(current => [...current, nextRow]); setNextRow(nextRow + 1); }}>Add check</button><button className="primary" disabled={!!selected?.retiredAt}>{editing ? 'Save repository' : 'Add repository'}</button>
      {editing && <button type="button" onClick={() => { setEditing(undefined); setRows([]); }}>Cancel edit</button>}
      {editing && selected && (selected.revision ?? 1) !== editing.revision && <button type="button" onClick={() => {
        if (window.confirm('Discard your draft and load the latest repository revision?')) edit(selected);
      }}>Reload current revision</button>}</div>
  </fieldset></form></>;
}

function ObservationPage({ runId, workspace, disconnected }: { runId: string; workspace?: Workspace; disconnected: boolean }) {
  const run = workspace?.runs.find(item => item.id === runId);
  const noop: Action = async () => null;
  return <main className="observation-page"><h1>Observation view</h1>
    {disconnected && <p role="alert" className="error">Connection lost. Retained activity may be out of date. Reconnecting…</p>}
    {run && workspace ? <ObservedRun run={run} workspace={workspace} action={noop} /> : <p>{workspace ? 'Run not found.' : 'Loading saved work…'}</p>}</main>;
}
function ObservedRun({ run, workspace, action }: { run: NonNullable<Workspace['runs'][number]>; workspace: Workspace; action: Action }) {
  const view = useRunObservation(run.id);
  return <><p>{run.taskSnapshot.title}</p><div className="run-layout"><div className="run-main"><AgentWall run={run} workspace={workspace} view={view} action={action} busy={false} readOnly /></div>
    <aside className="run-side"><CoordinatorPanel run={run} workspace={workspace} view={view} action={action} busy={false} readOnly /></aside></div></>;
}

function BriefEditor({ task, workspace, action, busy, draft, setDraft, locked }: { task: Task; workspace: Workspace; action: Action; busy: boolean; draft?: Draft; setDraft: (draft?: Draft) => void; locked: boolean }) {
  const brief = draft?.brief ?? task.brief; const criteria = draft?.criteria ?? task.criteria.join('\n');
  const revisions = workspace.requirementRevisions.filter(item => item.taskId === task.id).toReversed();
  const [repository, setRepository] = useState(task.repositoryId ?? '');
  const selectedRepository = workspace.repositories.find(repo => repo.id === repository);
  const update = (next: Draft) => setDraft(next.brief === task.brief && next.criteria === task.criteria.join('\n') ? undefined : next);
  return <div className="ticket-columns"><section className="card"><div className="section-title"><h2>Working brief</h2><span className="muted">{draft ? 'Unsaved changes' : `Revision ${task.revision}`}</span></div><p className="muted">Your local requirements. Saved separately from Jira.</p>
    <form onSubmit={async event => {
      event.preventDefault();
      const saved = await action(`/tasks/${task.id}/refinement`, { revision: task.revision, sourceRevision: task.sourceRevision, brief, criteria: criteria.split('\n').map(value => value.trim()).filter(Boolean) }, 'Brief saved.');
      if (saved) { await awaitWorkspace(value => value.tasks.some(item => item.id === task.id && item.revision === saved.revision)); setDraft(undefined); }
    }}><fieldset disabled={busy}><label>Local brief<textarea rows={6} value={brief} onChange={event => update({ brief: event.target.value, criteria })} /></label><label>Acceptance criteria<span className="hint">One criterion per line</span><textarea rows={5} value={criteria} onChange={event => update({ brief, criteria: event.target.value })} /></label>
      <div className="form-actions"><button className="primary">Save brief</button><button type="button" disabled={!draft} onClick={() => setDraft(undefined)}>Discard changes</button></div></fieldset></form>
    <section aria-label="Accepted brief history"><h3>Accepted brief history</h3>
      {!revisions.length && <p className="muted">No accepted brief saved yet.</p>}
      <ol>{revisions.map(item => <li key={item.revision}><strong>Revision {item.revision}{item === revisions[0] ? ' · Current accepted' : ''}</strong>
        <small> · {item.origin} · source revision {item.sourceRevision} · {new Date(item.createdAt).toLocaleString()}</small>
        <p>{item.brief || 'Empty brief'}</p><ul>{item.criteria.map((criterion, index) => <li key={index}>{criterion}</li>)}</ul>
        {item.decisionNote && <p>Decision: {item.decisionNote}</p>}</li>)}</ol>
    </section>
  </section><div><section className="card"><h2>Target repository</h2><form onSubmit={async event => {
    event.preventDefault(); await action(`/tasks/${task.id}/repository`, { repositoryId: repository, revision: task.revision }, 'Target repository saved.');
  }}><fieldset disabled={busy || locked}><label>Repository<select value={repository} onChange={event => setRepository(event.target.value)}><option value="">Select repository</option>{workspace.repositories.filter(repo => !repo.retiredAt || repo.id === repository).map(repo => <option key={repo.id} value={repo.id} disabled={!!repo.retiredAt}>{repo.name}{repo.retiredAt ? ' · Retired' : ''}</option>)}</select></label><button disabled={!repository || !!selectedRepository?.retiredAt}>Save target</button></fieldset></form>
    {selectedRepository && <div className="target-details"><p>{repository === task.repositoryId ? 'Saved target' : 'Selected target (not saved yet)'}: <strong>{selectedRepository.name}{selectedRepository.retiredAt ? ' · Retired' : ''}</strong></p>
      {selectedRepository.retiredAt && <p className="callout">Selected repository retired. Choose another target before Run.</p>}
      <p>Target branch: <strong>{selectedRepository.targetBranch}</strong> · Branch prefix: {selectedRepository.branchPrefix ?? 'codex/crewroom/'}</p>
      <p>PR publication {selectedRepository.publicationAvailable ? 'available' : 'unavailable (no Bitbucket connection)'}</p>
      <p>Checks: {selectedRepository.checks?.length ?? 0}</p>
      <ul>{selectedRepository.checks?.map(check => <li key={check.id}>{check.id} · {check.required ? 'required' : 'optional'} · <code>{check.command} {check.args.join(' ')}</code></li>)}</ul>
    </div>}{!workspace.repositories.length && <p className="muted">Add a repository using Repositories in the sidebar.</p>}</section>
    <section className="card source"><h2>Source description</h2><p>{task.description || 'No source description.'}</p>{task.sourceUrl && <a href={task.sourceUrl} target="_blank" rel="noreferrer">Open in Jira <ArrowUpRight size={14} /></a>}<small>Source revision {task.sourceRevision}</small></section></div></div>;
}

function TicketDependencies({ task, workspace, action, busy }: { task: Task; workspace: Workspace; action: Action; busy: boolean }) {
  const [candidate, setCandidate] = useState('');
  const available = workspace.tasks.filter(item => item.id !== task.id && !(task.dependsOn ?? []).includes(item.id));
  const save = async (dependsOn: string[]) => {
    const result = await action(`/tasks/${task.id}/dependencies`, { dependencyRevision: task.dependencyRevision ?? 0, dependsOn, requestId: crypto.randomUUID() }, 'Dependencies saved.');
    if (result) setCandidate('');
  };
  return <section className="card dependencies" aria-label="Ticket dependencies"><h2>Depends on</h2>
    <p className="muted">Local declarations survive Jira refresh. They apply to queued plans at scheduling time; a run already started is unaffected. Each ticket needs a provider-verified PR merge. For the same repository configuration, the checkout target must contain its merge commit.</p>
    {(task.dependsOn ?? []).length ? <ul>{task.dependsOn!.map(id => {
      const dependency = workspace.tasks.find(item => item.id === id);
      const delivery = workspace.dependencyStates?.[id];
      return <li key={id}><strong>{dependency?.key ?? id}</strong> · {delivery?.state ?? 'unknown'}{delivery?.reason && <> · {delivery.reason}</>}
        <button type="button" disabled={busy} aria-label={`Remove dependency ${dependency?.key ?? id}`} onClick={() => void save(task.dependsOn!.filter(item => item !== id))}>Remove</button></li>;
    })}</ul> : <p>No dependencies declared.</p>}
    <div className="dependency-add"><label>Add dependency<select value={candidate} onChange={event => setCandidate(event.target.value)} disabled={busy}>
      <option value="">Select ticket</option>{available.map(item => <option key={item.id} value={item.id}>{item.key} · {item.title}</option>)}</select></label>
      <button type="button" disabled={busy || !candidate || (task.dependsOn ?? []).length >= 12} onClick={() => void save([...(task.dependsOn ?? []), candidate])}>Add dependency</button></div>
  </section>;
}

function Ticket({ task, workspace, action, busy, draft, setDraft, route }: { route: TicketLocation; task: Task; workspace: Workspace; action: Action; busy: boolean; draft?: Draft; setDraft: (draft?: Draft) => void }) {
  const runs = workspace.runs.filter(run => run.taskId === task.id);
  const openRun = runs.find(run => !['closed', 'cancelled', 'completed'].includes(run.status));
  const run = runs.find(run => run.id === route.runId) ?? openRun ?? runs.at(-1);
  const capturedRepository = run && workspace.repositorySnapshots?.[run.id];
  const agents = workspace.activity.filter(agent => agent.runId === run?.id);
  const activity = agents.some(agent => agent.state === 'ownership_unconfirmed') ? 'Ownership unconfirmed' : agents.some(agent => agent.state === 'observation_degraded') ? 'Observation degraded · Process live' : agents.some(agent => agent.state === 'executing') ? `${agents.filter(agent => agent.state === 'executing').length} agents executing` : 'No agents executing';
  const [adoptionReason, setAdoptionReason] = useState('');
  const [queueRoleId, setQueueRoleId] = useState(workspace.roles.find(role => role.id === 'coordinator' && role.enabled && role.model)?.id ?? workspace.roles.find(role => role.enabled && role.model)?.id ?? workspace.roles.find(role => role.id === 'coordinator' && role.enabled)?.id ?? workspace.roles.find(role => role.enabled)?.id ?? '');
  const claimed = agents.some(agent => agent.state !== 'admitted');
  const [savedTab, setSavedTab] = useState<'coordinator' | 'evidence' | 'brief'>(() => {
    try { const saved = localStorage.getItem('crewroom.runTab'); if (saved === 'evidence' || saved === 'brief') return saved; } catch {}
    return 'coordinator';
  });
  const tab = route.tab ?? savedTab;
  const setTab = (value: typeof tab) => { setSavedTab(value); navigateTicket({ ...route, runId: run?.id, tab: value }); try { localStorage.setItem('crewroom.runTab', value); } catch {} };
  useEffect(() => { if (run && !route.runId) navigateTicket({ ...route, runId: run.id, tab }, true); }, [run?.id, route.runId, tab]);
  const createRun: Action = async (path, body, success) => {
    const created = await action(path, body, success);
    if (created) {
      await awaitWorkspace(value => value.runs.some(item => item.id === created.id));
      if (parseTicketLocation(location.hash).taskId === task.id) navigateTicket({ taskId: task.id, runId: created.id, tab });
    }
    return created;
  };
  const queueBlockers = [!!openRun && 'close the current run', !task.repositoryId && 'choose a target repository', !!workspace.repositories.find(repo => repo.id === task.repositoryId)?.retiredAt && 'choose another target repository (selected repository retired)', !task.brief.trim() && 'write a brief', !task.criteria.length && 'add acceptance criteria', task.needsReacceptance && 'accept the changed source requirements', draft && 'save or discard your brief changes'].filter(Boolean) as string[];
  const researchBlockers = [!!openRun && 'close the current run', !!workspace.repositories.find(repo => repo.id === task.repositoryId)?.retiredAt && 'choose another target repository (selected repository retired)', !task.brief.trim() && 'write a brief', !task.criteria.length && 'add acceptance criteria', task.needsReacceptance && 'accept the changed source requirements', draft && 'save or discard your brief changes'].filter(Boolean) as string[];
  const heading = <>
    <div className="ticket-heading"><div><div className="ticket-key">{task.key}<span>{task.source === 'jira' ? task.sourceSite : 'Local ticket'}</span></div><h1>{task.title}</h1></div>
      <div className="queue"><form onSubmit={event => {
        event.preventDefault();
        const snapshot = openRun?.taskSnapshot ?? task;
        const launch = openRun?.continuedFrom ? { ...openRun.launch!, team: openRun.launch!.team.map(member => member.coordinator ? { ...member, roleId: queueRoleId } : member) } : { team: [{ name: 'Coordinator', roleId: queueRoleId, coordinator: true, input: JSON.stringify({ brief: snapshot.brief, criteria: snapshot.criteria }) }], repository: { mode: 'new' } };
        if (openRun) void action(`/runs/${openRun.id}/launch`, { requestId: crypto.randomUUID(), launch }, 'Launch details saved.');
        else void createRun(`/tasks/${task.id}/runs`, { revision: task.revision, requestId: crypto.randomUUID(), launch }, 'Plan queued. Coding starts automatically when eligible.');
      }}><label>Role for queued coordinator<select value={queueRoleId} onChange={event => setQueueRoleId(event.target.value)} disabled={busy || !!openRun && openRun.position === undefined}>{workspace.roles.filter(role => role.enabled).map(role => <option key={role.id} value={role.id}>{role.name} · {role.model}</option>)}</select></label>
        <small>Repository mode: {openRun?.continuedFrom ? 'continue retained branch' : 'new branch'} · queued coding starts automatically when eligible</small>
        <button className="primary" disabled={busy || !queueRoleId || (openRun ? openRun.position === undefined : queueBlockers.length > 0)}>{openRun?.position !== undefined ? 'Save launch details' : openRun ? 'Run recorded' : 'Queue run'}</button></form>
        {!openRun && <><button disabled={busy || !!draft || !!task.needsReacceptance || !workspace.connections.runner} onClick={() => createRun(`/tasks/${task.id}/refinement-runs`, { revision: task.revision, purpose: 'explore', requestId: crypto.randomUUID() }, 'Explore coordinator started.')}>Explore</button><button disabled={busy || !!draft || !!task.needsReacceptance || !workspace.connections.runner} onClick={() => createRun(`/tasks/${task.id}/refinement-runs`, { revision: task.revision, purpose: 'grill', requestId: crypto.randomUUID() }, 'Grill coordinator started.')}>Grill</button></>}
        {!openRun && <button disabled={busy || researchBlockers.length > 0} onClick={() => createRun(`/tasks/${task.id}/research-runs`, { revision: task.revision, requestId: crypto.randomUUID() }, 'Research queued. Start the coordinator when ready.')}>Queue research</button>}
        {!openRun && queueBlockers.length > 0 && <small className="muted">To queue: {queueBlockers.join(', ')}.</small>}
        {openRun?.position !== undefined && <small className="muted">Queued plan, not running. {openRun.launch ? 'Launch details saved.' : 'Launch details missing.'} Hold reason: {workspace.queue.plans.find(plan => plan.id === openRun.id)?.holdReason ?? 'Eligible · scheduling'}</small>}</div></div>
    <TicketDependencies task={task} workspace={workspace} action={action} busy={busy} />
    <dl className="status-strip"><div><dt>{task.source === 'jira' ? 'Jira status' : 'Source status'}</dt><dd>{task.jiraStatus}</dd></div><div><dt>Workflow</dt><dd>{run ? runLabel(run) : 'Refining'}</dd></div><div><dt>Agent activity</dt><dd>{activity}</dd></div></dl>
    {route.runId && !runs.some(item => item.id === route.runId) && <p role="status" className="callout">That run is unavailable for this ticket. Showing {run ? 'the default run' : 'the ticket brief'}.</p>}
    {!!runs.length && <label>Run history<select aria-label="Run history" value={run?.id ?? ''} onChange={event => navigateTicket({ taskId: task.id, runId: event.target.value, tab })}>{runs.toReversed().map(item => <option key={item.id} value={item.id}>{item.purpose ?? 'coding'} · {item.status} · {item.startedAt ? `Started ${new Date(item.startedAt).toLocaleString()}` : 'Start unknown'} · Created {new Date(item.createdAt).toLocaleString()} · {runOutcome(item)}{workspace.repositorySnapshots?.[item.id] ? ` · ${workspace.repositorySnapshots[item.id].name}` : ''}</option>)}</select></label>}
    {run && (run.continuedFrom || run.continuedBy || run.desiredState === 'cancelled' && (run.purpose ?? 'coding') === 'coding') && <RunContinuation key={run.id} run={run} task={task} workspace={workspace} roleId={queueRoleId} action={createRun} busy={busy || !!openRun && openRun.id !== run.id} unsaved={!!draft} />}
    {runs.length > 1 && capturedRepository && <p className="muted run-history-repository" aria-label="Run history repository">This run prepared <strong>{capturedRepository.name}</strong> · {capturedRepository.path} · target {capturedRepository.targetBranch} · prefix {capturedRepository.branchPrefix ?? 'codex/crewroom/'} · checks {(capturedRepository.checks ?? []).map(check => check.id).join(', ') || 'none'}. This is the captured configuration, not the latest setting.</p>}
    {run && (openRun?.id === run.id || run.status === 'completed')
      && (run.taskSnapshot.brief !== task.brief || JSON.stringify(run.taskSnapshot.criteria) !== JSON.stringify(task.criteria)
        || run.taskSnapshot.sourceRevision !== task.sourceRevision)
      && <section className="callout" aria-label="Requirement adoption"><p>Revision {task.revision} awaits an adoption decision. This run retains brief revision {run.taskRevision}.</p>
      <label>Adoption reason{claimed ? ' (required for active work)' : ' (optional for queued work)'}<input value={adoptionReason} onChange={event => setAdoptionReason(event.target.value)} /></label>
      <button type="button" disabled={busy || !!draft || !!task.needsReacceptance || (claimed && !adoptionReason.trim())} onClick={async () => {
        if (await action(`/runs/${run.id}/requirements/adopt`, { requestId: crypto.randomUUID(), revision: task.revision, ...(adoptionReason.trim() ? { reason: adoptionReason.trim() } : {}) }, 'Requirements adopted.')) setAdoptionReason('');
      }}>Adopt revision {task.revision}</button></section>}
    {run && <p className="muted">{run.purpose === 'explore' || run.purpose === 'grill' ? `${run.purpose === 'explore' ? 'Explore' : 'Grill'} based on revision ${run.taskRevision}${run.adoptions?.length ? ` · last adoption: ${run.adoptions.at(-1)?.reason ?? 'operator decision'}` : ''}` : `Run adopted revision ${run.taskRevision}${run.adoptions?.length ? ` · last decision: ${run.adoptions.at(-1)?.reason ?? 'queued adoption'}` : ' · queued snapshot'}`}.</p>}
    {task.needsReacceptance && <p className="callout">Source requirements changed. Compare the source below with your brief, then save the accepted requirements.</p>}
    {workspace.jira?.state === 'connected' && task.source === 'jira' && !workspace.jira.taskIds.includes(task.id) && <p className="callout">This ticket is outside the current Jira filter. Saved work is retained.</p>}
    {run?.sourceConflict && <p className="callout">{run.sourceConflict}</p>}
    {workspace.queue.slot && workspace.queue.slot.runId === run?.id && <p className="callout">This ticket holds the coding slot · {workspace.queue.slot.reason}.</p>}
  </>;
  if (!run) return <>{heading}<RefinementPanel task={task} workspace={workspace} action={action} busy={busy} /><BriefEditor task={task} workspace={workspace} action={action} busy={busy} draft={draft} setDraft={setDraft} locked={false} /></>;
  return <>{heading}<RefinementPanel task={task} workspace={workspace} action={action} busy={busy} /><RunTicket key={run.id} attemptId={route.runId === run.id ? route.attemptId : undefined} run={run} task={task} workspace={workspace} action={action} busy={busy} draft={draft} setDraft={setDraft} heading={null} tab={tab} setTab={setTab} /></>;
}

function RunTicket({ run, task, workspace, action, busy, draft, setDraft, heading, tab, setTab, attemptId }: { attemptId?: string; run: NonNullable<Workspace['runs'][number]>; task: Task; workspace: Workspace; action: Action; busy: boolean; draft?: Draft; setDraft: (draft?: Draft) => void; heading: React.ReactNode; tab: 'coordinator' | 'evidence' | 'brief'; setTab: (tab: 'coordinator' | 'evidence' | 'brief') => void }) {
  const view = useRunObservation(run.id);
  const tabs = [['coordinator', 'Coordinator'], ['evidence', run.purpose === 'research' ? 'Research evidence' : 'Change & evidence'], ['brief', 'Brief']] as const;
  return <>{heading}<div className="run-layout">
    <div className="run-main">{view.observation?.history && <AttemptHistory run={run} history={view.observation.history} selectedId={attemptId} tab={tab} />}
      {!view.observation?.history?.attempts.some(item => item.attemptId === attemptId) && <AgentWall run={run} workspace={workspace} view={view} action={action} busy={busy} />}</div>
    <aside className="run-side" aria-label="Coordinator and evidence">
      <div role="tablist" aria-label="Run details" className="tabs">{tabs.map(([id, label]) => <button key={id} role="tab" id={`tab-${id}`} aria-selected={tab === id} tabIndex={tab === id ? 0 : -1} onKeyDown={event => {
        const current = tabs.findIndex(([value]) => value === id);
        const next = event.key === 'ArrowRight' ? (current + 1) % tabs.length : event.key === 'ArrowLeft' ? (current + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : undefined;
        if (next === undefined) return; event.preventDefault(); setTab(tabs[next][0]); document.getElementById(`tab-${tabs[next][0]}`)?.focus();
      }} aria-controls={`panel-${id}`} className={tab === id ? 'active' : ''} onClick={() => setTab(id)}>{label}{id === 'brief' && draft ? ' •' : ''}</button>)}</div>
      <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} tabIndex={0} className="tab-panel">
        {tab === 'coordinator' && <CoordinatorPanel run={run} workspace={workspace} view={view} action={action} busy={busy} />}
        {tab === 'evidence' && (run.purpose === 'explore' || run.purpose === 'grill' ? <p className="muted">Refinement has no coding or Jira delivery. Proposals and notes remain in Explore &amp; Grill history.</p> : <><JiraUpdates workspace={workspace} runId={run.id} action={action} busy={busy} /><ResultPanel runId={run.id} load={api} action={action} busy={busy} repositories={workspace.repositories} /></>)}
        {tab === 'brief' && <BriefEditor task={task} workspace={workspace} action={action} busy={busy} draft={draft} setDraft={setDraft} locked={!['closed', 'cancelled', 'completed'].includes(run.status)} />}
      </div>
    </aside>
  </div></>;
}

createRoot(document.getElementById('root')!).render(<App />);
