import { readRunHistory } from './run-history';
import { RunContinuation } from './run-continuation';
import express from 'express';
import { createServer } from 'node:http';
import { realpathSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { homedir } from 'node:os';
import { z, ZodError } from 'zod';
import { Workbench, Conflict, NotFound } from './workbench';
import { repositoryUpdateSchema, validateRepository, RepositoryValidationError } from './repository-configuration';
import { RoleCatalog, RoleConflict } from './roles';
import { Learning } from './learning';
import { RuntimeSupervisor, type RuntimeConfig } from './runtime/supervisor';
import { acquireController } from './runtime/processes';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import { CodingScheduler, prepareCodingRepository } from './coding-scheduler';
import { Coding } from './coding';
import { Research } from './research';
import { Results } from './results';
import { Checks } from './checks';
import { JiraClient, type JiraConnection } from './jira-client';
import { JiraSync } from './jira-sync';
import { JiraDelivery } from './jira-delivery';
import { Publication, type PublicationConnection } from './publication';
import { TargetRefresh } from './target-refresh';
import { LiveHub } from './live';
import { TerminalHub } from './terminals';
import type { IncomingMessage } from 'node:http';

export async function startServer(options: { port: number; dataDir: string; serveUi?: boolean; production?: boolean; rolePath?: string; runtimeConfig?: RuntimeConfig; observation?: { socket: string; root: string }; jira?: JiraConnection; publication?: PublicationConnection[] }) {
  const jiraClient = options.jira ? new JiraClient(options.jira) : undefined;
  const controller = await acquireController(join(options.dataDir, 'controller'));
  let store: Workbench;
  try { store = new Workbench(join(options.dataDir, 'workflow.sqlite')); }
  catch (error) { await controller.close(); throw error; }
  const roles = new RoleCatalog(options.rolePath ?? join(homedir(), '.config/crewroom/roles.json'));
  const learning = new Learning(store, roles);
  try { learning.reconcile(); } catch (error) { store.close(); await controller.close(); throw error; }
  /** Role reads that feed admission first complete any approved role save interrupted by a transient failure. */
  const admissionRoles = () => { try { learning.reconcile(); } catch { /* A failed save stays visible as pending; admission uses the saved catalog. */ } return roles.read(); };
  const repositoryWork = new RepositoryWork(store, join(options.dataDir, 'worktrees'), controller);
  const evidence = new Evidence(store, repositoryWork, join(options.dataDir, 'artifacts'));
  const coding = new Coding(store, repositoryWork, evidence);
  const research = new Research(store, repositoryWork, evidence);
  const checks = options.runtimeConfig?.commandEngine ? new Checks(store, repositoryWork, evidence, options.runtimeConfig.root, options.runtimeConfig.commandEngine) : undefined;
  let publication: Publication;
  try { publication = new Publication(store, repositoryWork, evidence, join(options.dataDir, 'publication'), options.publication ?? []); }
  catch (error) { store.close(); await controller.close(); throw error; }
  const results = new Results(store, repositoryWork, evidence, publication, research);
  const targetRefresh = new TargetRefresh(store, repositoryWork, evidence, publication);
  const continuation = new RunContinuation(store, repositoryWork);
  const jira = jiraClient ? new JiraSync(store, jiraClient) : undefined;
  const jiraDelivery = jiraClient ? new JiraDelivery(store, jiraClient, evidence, research) : undefined;
  let stopping = false;
  let promotionReady = false;
  const observedStatus = async (runId: string) => {
    if (!store.repositoryWork(runId)) return undefined;
    try { return await repositoryWork.observedStatus(runId); }
    catch (error) { return error instanceof Error ? error : new Error('Repository observation is unavailable.'); }
  };
  const app = express();
  const server = createServer(app);
  let runtime: RuntimeSupervisor | undefined;
  let scheduler: CodingScheduler | undefined;
  app.disable('x-powered-by');
  app.use((req, res, next) => {
    if (stopping) { res.status(503).json({ error: 'Crewroom is stopping. Saved work is retained.' }); return; }
    try { controller.assertCurrent(); }
    catch { res.status(503).json({ error: 'Controller authority was lost. Saved work is retained.' }); return; }
    if (!promotionReady && !['GET', 'HEAD'].includes(req.method)) { res.status(503).json({ error: 'Repository promotion recovery is still reconciling.' }); return; }
    const host = `127.0.0.1:${req.socket.localPort}`;
    if (req.headers.host !== host || (req.headers.origin && req.headers.origin !== `http://${host}`) || req.headers['sec-fetch-site'] === 'cross-site') {
      res.status(403).json({ error: 'Use the local Crewroom origin.' }); return;
    }
    if (!['GET', 'HEAD'].includes(req.method) && (req.headers.origin !== `http://${host}` || !req.is('application/json'))) {
      res.status(403).json({ error: 'Mutations require same-origin JSON requests.' }); return;
    }
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Frame-Options', 'DENY');
    res.setHeader('Cache-Control', 'no-store');
    next();
  });
  let live: LiveHub | undefined;
  app.use((req, res, next) => { if (req.method === 'POST') res.once('finish', () => live?.changed()); next(); });
  app.use(express.json({ limit: '2mb' }));
  const publicationPaths = new Set((options.publication ?? []).map(connection => realpathSync(connection.repositoryPath)));
  const workspaceView = () => {
    const workspace = store.snapshot();
    const activity = workspace.runs.flatMap(run => {
      const state = store.runtime(run.id);
      return state.assignments.map(assignment => {
        const attempt = state.attempts.find(attempt => attempt.id === assignment.currentAttemptId)!;
        return { runId: run.id, assignmentName: assignment.name, ompPid: attempt.ompPid,
          state: runtime?.isDegraded(attempt.id) ? 'observation_degraded' : runtime?.observes(attempt.id) || attempt.state === 'exited' || attempt.state === 'admitted' ? attempt.state : 'ownership_unconfirmed' };
      });
    });
    return { ...workspace, repositories: workspace.repositories.map(repository => {
      let path = repository.path;
      try { path = realpathSync(path); } catch { /* Preserve saved configurations whose checkout is temporarily unavailable. */ }
      return { ...repository, publicationAvailable: publicationPaths.has(path) };
    }), roles: roles.read(), connections: { jira: jira?.snapshot().state === 'connected', runner: !!runtime }, jira: jira?.snapshot(), jiraUpdates: store.jiraUpdates(), activity };
  };
  /** Running and finished container commands, derived from durable host events. */
  const commandView = (runId: string) => {
    const commands = new Map<string, { attemptId: string; toolCallId: string; commandId: string; state: 'running' | 'exited' | 'cancelled'; exitCode?: number }>();
    for (const { attemptId, event } of store.commandEvents(runId)) {
      if (typeof event.toolCallId !== 'string' && event.type !== 'tool_cancelled') continue;
      if (event.type === 'tool_started' || event.type === 'tool_receipt_reused') commands.set(event.toolCallId, { attemptId, toolCallId: event.toolCallId, commandId: String(event.commandId ?? 'command'), state: 'running' });
      else if (event.type === 'tool_exited' && commands.has(event.toolCallId)) Object.assign(commands.get(event.toolCallId)!, { state: 'exited', exitCode: event.result?.exitCode });
    }
    return [...commands.values()];
  };
  const runView = (runId: string) => ({ teamProposals: store.teamProposals(runId), ...(runtime?.snapshot(runId) ?? store.runtime(runId)), commands: commandView(runId), history: readRunHistory(store, runId) });
  app.get('/api/workspace', (_req, res) => res.json(workspaceView()));
  app.post('/api/jira/sync', async (_req, res) => { if (!jira) { res.status(409).json({ error: 'Jira connection is not configured.' }); return; } res.json(await jira.refresh()); });
  app.post('/api/jira/reconcile', async (_req, res) => { if (!jiraDelivery) throw new Conflict('Jira connection is not configured.'); await jiraDelivery.flush(); for (const run of store.snapshot().runs) runtime?.notify(run.id); res.json(store.jiraUpdates()); });
  app.post('/api/runs/:id/publication/reconcile', async (req, res) => { store.getRun(req.params.id); await publication.flush(); runtime?.notify(req.params.id); res.json(await publication.view(req.params.id)); });
  app.get('/api/runs/:id/feedback', async (req, res) => { store.getRun(req.params.id); res.json(await publication.observeFeedback(req.params.id) ?? null); live?.changed(); });
  app.post('/internal/publication/:operation', async (req, res) => {
    if (!runtime || !['prepare', 'status'].includes(req.params.operation)) throw new Conflict('Publication operation is unavailable.');
    const result = await runtime.publicationOperation(req.headers.authorization, req.body, req.params.operation as 'prepare' | 'status');
    if (req.params.operation === 'prepare') runtime.notify(store.getAttempt(req.body.attemptId).runId);
    res.json(result);
  });
  app.post('/internal/jira/:operation', async (req, res) => {
    if (!runtime || !['report', 'updates'].includes(req.params.operation)) throw new Conflict('Jira agent operation is unavailable.');
    const result = await runtime.jiraOperation(req.headers.authorization, req.body, req.params.operation as 'report' | 'updates');
    if (req.params.operation === 'report') runtime.notify(store.getAttempt(req.body.attemptId).runId);
    res.json(result);
  });
  app.post('/api/tasks', (req, res) => res.status(201).json(store.createTask(req.body)));
  app.post('/api/import/jira', (req, res) => res.json(store.importJira(req.body)));
  app.post('/api/repositories', async (req, res) => {
    const replay = store.repositoryReplay(req.body);
    if (replay) { res.json(replay); return; }
    const validated = await validateRepository(req.body, path => store.isOwnedRepositoryWorktree(path));
    res.status(201).json(store.saveRepository(validated));
  });
  app.post('/api/repositories/:id', async (req, res) => {
    const value = repositoryUpdateSchema.parse(req.body);
    const replay = store.repositoryUpdateReplay(req.params.id, value);
    if (replay) { res.json(replay); return; }
    const { revision, ...configuration } = value;
    const validated = await validateRepository(configuration, path => store.isOwnedRepositoryWorktree(path));
    res.json(store.updateRepository(req.params.id, validated, value));
    scheduler?.kick();
  });
  app.post('/api/repositories/:id/retire', (req, res) => { const saved = store.retireRepository(req.params.id, req.body); scheduler?.kick(); res.json(saved); });
  app.post('/api/roles', (req, res) => { const result = roles.create(req.body); scheduler?.kick(); res.status(201).json(result); });
  app.post('/api/roles/:id', (req, res) => { const result = roles.update(req.params.id, req.body); scheduler?.kick(); res.json(result); });
  app.post('/api/tasks/:id/refinement', (req, res) => res.json(store.refine(req.params.id, req.body)));
  app.get('/api/tasks/:id/revisions', (req, res) => res.json(store.requirementRevisions(req.params.id)));
  app.post('/api/tasks/:id/repository', (req, res) => res.json(store.assignRepository(req.params.id, req.body)));
  app.post('/api/tasks/:id/dependencies', (req, res) => { const task = store.setDependencies(req.params.id, req.body); scheduler?.kick(); res.json(task); });
  app.post('/api/tasks/:id/notes', (req, res) => res.status(201).json(store.addNote(req.params.id, req.body)));
  app.post('/api/tasks/:id/runs', (req, res) => { const run = store.queue(req.params.id, req.body, admissionRoles()); scheduler?.kick(); res.status(201).json(run); });
  app.post('/api/queue/order', (req, res) => { const queue = store.reorderQueue(req.body); scheduler?.kick(); res.json(queue); });
  app.post('/api/queue/:id/remove', (req, res) => { const run = store.cancelQueued(req.params.id, req.body); scheduler?.kick(); res.json(run); });
  app.post('/api/runs/:id/launch', (req, res) => { const run = store.setQueueLaunch(req.params.id, req.body, admissionRoles()); scheduler?.kick(); res.json(run); });
  app.post('/api/runs/:id/continue-later', async (req, res) => {
    const perform = () => continuation.queue(req.params.id, req.body, admissionRoles());
    const run = runtime ? await runtime.operatorTransition(req.params.id, perform) : await perform();
    scheduler?.kick(); res.status(201).json(run);
  });
  app.post('/api/tasks/:id/research-runs', async (req, res) => {
    const task = store.getTask(req.params.id);
    const repository = task.repositoryId ? store.snapshot().repositories.find(item => item.id === task.repositoryId) : undefined;
    if (repository?.retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
    const source = repository ? await repositoryWork.pinResearch(repository) : undefined;
    res.status(201).json(store.queueResearch(task.id, req.body, source));
  });
  app.post('/api/tasks/:id/refinement-runs', async (req, res) => {
    if (!runtime) throw new Conflict('Runner integration is unavailable.');
    const run = store.queueRefinement(req.params.id, req.body);
    if (run.status === 'awaiting_runner' && !store.runtime(run.id).assignments.length) await runtime.startRefinement(run.id, admissionRoles());
    res.status(201).json(store.getRun(run.id));
  });
  app.post('/api/refinement-proposals/:id/decision', (req, res) => res.json(store.decideRefinement(req.params.id, req.body)));
  app.post('/api/runs/:id/close', async (req, res) => {
    const run = store.getRun(req.params.id);
    if (run.purpose !== 'explore' && run.purpose !== 'grill') throw new Conflict('Only refinement runs can be closed.');
    if (run.status === 'closed') { res.json(run); return; }
    if (runtime && run.status !== 'cancelled' && run.status !== 'paused') await runtime.pause(run.id);
    res.json(store.closeRefinement(run.id));
  });
  app.post('/api/runs/:id/steering', (req, res) => { const message = store.steer(req.params.id, req.body); runtime?.notify(req.params.id); res.status(201).json(message); });
  app.post('/api/runs/:id/specialists', (req, res) => {
    if (!runtime) throw new Conflict('Runner integration is unavailable; specialist models cannot be validated.');
    res.status(201).json(runtime.insertSpecialist(req.params.id, req.body));
    scheduler?.kick();
  });
  app.post('/api/runs/:id/requirements/adopt', async (req, res) => {
    const adopted = store.adoptRequirements(req.params.id, req.body);
    if (runtime && adopted.status === 'pausing') await runtime.pause(adopted.id, true);
    runtime?.notify(adopted.id);
    res.json(store.getRun(adopted.id));
  });
  app.post('/api/runs/:id/repository', async (req, res) => res.json(await prepareCodingRepository(repositoryWork, evidence, req.params.id, req.body)));
  app.get('/api/runs/:id/result', async (req, res) => res.json(await results.read(req.params.id, await observedStatus(req.params.id))));
  app.get('/api/runs/:id/result/artifacts/:artifactId', (req, res) => res.json(results.artifact(req.params.id, req.params.artifactId)));
  app.get('/api/runs/:id/result/candidates/:candidateId/diff', async (req, res) => res.json(await results.diff(req.params.id, req.params.candidateId)));
  app.post('/internal/research/:operation', async (req, res) => {
    if (!runtime || !['submit', 'read', 'search', 'complete'].includes(req.params.operation)) throw new Conflict('Research operation is unavailable.');
    const result = await runtime.researchOperation(req.headers.authorization, req.body, req.params.operation as 'submit' | 'read' | 'search' | 'complete');
    if (req.params.operation === 'complete') runtime.notify(store.getAttempt(req.body.attemptId).runId);
    res.json(result);
  });
  app.get('/api/runs/:id/coding', async (req, res) => res.json({ record: store.codingRecord(req.params.id), assessment: await evidence.assess(req.params.id, await observedStatus(req.params.id)), checksAvailable: !!checks }));
  app.post('/api/runs/:id/check-environment', (req, res) => res.json(evidence.correctCheckEnvironment(req.params.id, req.body)));
  app.post('/api/runs/:id/target-refresh', async (req, res) => {
    const perform = () => targetRefresh.refresh(req.params.id, req.body);
    const result = runtime ? await runtime.operatorTransition(req.params.id, perform) : await perform();
    runtime?.notify(req.params.id); res.json(result);
  });
  app.get('/api/runs/:id/repository', async (req, res) => res.json(await repositoryWork.observedStatus(req.params.id)));
  app.get('/api/runs/:id/research', (req, res) => res.json(research.read(req.params.id)));
  app.post('/api/runs/:id/start', async (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runner integration is unavailable.' }); return; }
    const run = store.getRun(req.params.id);
    if (run.purpose === 'explore' || run.purpose === 'grill') {
      z.object({}).strict().parse(req.body);
      res.json(await runtime.startRefinement(run.id, admissionRoles()));
      return;
    }
    if (run.purpose === 'research') { res.json(await runtime.start(run.id, req.body, admissionRoles())); return; }
    store.assertCodingSlotAvailable(run.id);
    const baseHold = await repositoryWork.dependencyBaseHold(run.id);
    if (baseHold) throw new Conflict(baseHold);
    runtime.preflightStart(run.id, req.body, admissionRoles());
    const decided = store.claimCodingSlot(run.id, true);
    res.json(await scheduler!.launch(decided, req.body));
  });
  app.post('/api/runs/:id/pause', async (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runner integration is unavailable.' }); return; }
    res.json(await runtime.pause(req.params.id));
  });
  app.post('/api/runs/:id/assignments/:assignmentId/repair-allowance', (req, res) => { const result = store.grantRepairCycle(req.params.id, req.params.assignmentId, req.body); runtime?.notify(req.params.id); res.json(result); });
  app.post('/api/runs/:id/budget', (req, res) => { const result = store.setActiveLimit(req.params.id, req.body); runtime?.notify(req.params.id); res.json(result); });
  app.post('/api/runs/:id/cancel', async (req, res) => {
    const run = store.getRun(req.params.id);
    if (req.body?.requestId && (run.purpose ?? 'coding') === 'coding' && (run.position !== undefined || run.status === 'cancelled')) { const removed = store.cancelQueued(run.id, req.body); scheduler?.kick(); res.json(removed); return; }
    if (runtime) res.json(await runtime.cancel(req.params.id));
    else { store.setDesiredState(req.params.id, 'cancelled'); store.beginPause(req.params.id); res.json(store.finishPause(req.params.id)); }
  });
  app.post('/api/runs/:id/resume', async (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runner integration is unavailable.' }); return; }
    res.json(await runtime.resume(req.params.id));
  });
  app.get('/api/runs/:id/runtime', (req, res) => res.json(runView(req.params.id)));
  app.get('/api/runs/:id/events', (req, res) => {
    store.getRun(req.params.id);
    const after = req.query.after === undefined ? 0 : Number(req.query.after);
    const limit = req.query.limit === undefined ? 100 : Number(req.query.limit);
    if (!Number.isSafeInteger(after) || after < 0 || !Number.isSafeInteger(limit) || limit < 1 || limit > 200) throw new Conflict('Event cursor or page size is invalid.');
    res.json(store.agentEventPage(req.params.id, after, limit));
  });
  app.post('/internal/agent-message', async (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runtime is unavailable.' }); return; }
    res.json(await runtime.message(req.headers.authorization, req.body));
  });
  app.post('/internal/steering-receipt', (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runtime is unavailable.' }); return; }
    res.json(runtime.steeringReceipt(req.headers.authorization, req.body));
  });
  app.post('/internal/learning-proposal', (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runtime is unavailable.' }); return; }
    res.json(runtime.learningOperation(req.headers.authorization, req.body));
  });
  app.post('/internal/refinement-proposal', (req, res) => {
    if (!runtime) throw new Conflict('Runtime is unavailable.');
    res.json(runtime.refinementOperation(req.headers.authorization, req.body));
  });
  app.post('/api/learning/:id/decision', (req, res) => { const result = learning.decide(req.params.id, req.body); scheduler?.kick(); res.json(result); });
  for (const [path, operation] of [['agent-roles', 'roles'], ['team-proposal', 'propose']] as const) app.post(`/internal/${path}`, (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runtime is unavailable.' }); return; }
    const result = runtime.coordinatorOperation(req.headers.authorization, req.body, operation);
    if (operation === 'propose') runtime.notify(store.getAttempt(req.body.attemptId).runId);
    res.json(result);
  });
  for (const operation of ['status', 'output', 'read', 'search', 'shell', 'claim', 'edit', 'patch', 'conflict-continue', 'complete', 'check-prepare', 'check-finish', 'review', 'review-assign'] as const) app.post(`/internal/coding/${operation}`, async (req, res) => {
    if (!runtime) { res.status(409).json({ error: 'Runtime is unavailable.' }); return; }
    const result = await runtime.codingOperation(req.headers.authorization, req.body, operation);
    if (['claim', 'edit', 'patch', 'conflict-continue', 'complete', 'check-prepare', 'check-finish', 'review', 'review-assign'].includes(operation)) runtime.notify(store.getAttempt(req.body.attemptId).runId);
    res.json(result);
  });
  app.use('/api', (_req, res) => res.status(404).json({ error: 'API operation not found.' }));
  let vite: import('vite').ViteDevServer | undefined;
  try {
    if (options.serveUi !== false) {
      if (options.production) {
        app.use(express.static(resolve('dist')));
        app.get('/', (_req, res) => res.sendFile(resolve('dist/index.html')));
      } else {
        const { createServer: createVite } = await import('vite');
        vite = await createVite({ server: { middlewareMode: true, hmr: { server } }, appType: 'spa' });
        app.use(vite.middlewares);
      }
    }
    app.use((error: unknown, _req: express.Request, res: express.Response, _next: express.NextFunction) => {
      const status = error instanceof Conflict || error instanceof RoleConflict || error instanceof RepositoryValidationError ? 409 : error instanceof NotFound ? 404 : error instanceof ZodError || error instanceof SyntaxError ? 400 : 500;
      const message = error instanceof ZodError ? error.issues.map(issue => `${issue.path.join('.')}: ${issue.message}`).join('; ')
        : status !== 500 && error instanceof Error ? error.message : 'Unable to complete this operation. Saved work has been retained.';
      res.status(status).json({ error: message });
    });
    await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(options.port, '127.0.0.1', resolve); });
  } catch (error) { await vite?.close(); store.close(); await controller.close(); throw error; }
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing loopback address.');
  // Observation-only access to retained panes and logs, also available when no runner is configured.
  const panes = options.runtimeConfig ?? options.observation;
  const terminals = panes ? new TerminalHub(panes.socket, panes.root, pane => store.attemptForPane(pane)) : undefined;
  terminals?.onPaneExited(pane => runtime?.paneExited(pane));
  const loopback = `127.0.0.1:${address.port}`;
  live = new LiveHub(server, { store, workspace: workspaceView, run: runView, runtimeRoot: panes?.root, terminals,
    result: async runId => results.read(runId, await observedStatus(runId)) },
    (req: IncomingMessage) => req.headers.host === loopback && req.headers.origin === `http://${loopback}`);
  try {
    await targetRefresh.reconcile();
    controller.assertCurrent();
    promotionReady = true;
    if (options.runtimeConfig) {
      runtime = new RuntimeSupervisor(store, options.runtimeConfig, `http://127.0.0.1:${address.port}`, admissionRoles, repositoryWork, { coding, evidence, checks }, jiraDelivery, publication, learning, research);
      await runtime.initialize();
      scheduler = new CodingScheduler(store, repositoryWork, evidence, runtime, admissionRoles, () => live?.changed());
      runtime.onSchedulingChange(() => scheduler?.kick());
      scheduler.kick();
    }
  } catch (error) {
    live.close(); terminals?.close();
    await vite?.close(); await new Promise<void>(resolve => server.close(() => resolve())); store.close(); await controller.close(); throw error;
  }
  const syncTimer = jira ? setInterval(() => { if (!stopping) void jira.refresh(); }, 60000) : undefined;
  const deliveryTimer = jiraDelivery ? setInterval(() => { if (!stopping) void jiraDelivery.flush().then(() => { for (const run of store.snapshot().runs) runtime?.notify(run.id); }).catch(() => console.error('Jira delivery reconciliation failed; saved intents require inspection.')); }, 5000) : undefined;
  const publicationTimer = options.publication?.length ? setInterval(() => { if (!stopping) void publication.flush().then(() => { for (const run of store.snapshot().runs) runtime?.notify(run.id); }).catch(() => console.error('Publication reconciliation failed; saved intents require inspection.')); }, 15000) : undefined;
  const feedbackTimer = options.publication?.length ? setInterval(() => { if (!stopping) void Promise.all(store.publications()
    .filter(record => record.pr.state === 'confirmed' && !!record.pr.id).map(record => publication.observeFeedback(record.runId).catch(() => undefined)))
    .then(() => live?.changed()); }, 15000) : undefined;
  syncTimer?.unref(); deliveryTimer?.unref(); publicationTimer?.unref(); feedbackTimer?.unref();
  return { origin: `http://127.0.0.1:${address.port}`, async close() {
    stopping = true; clearInterval(syncTimer); clearInterval(deliveryTimer); clearInterval(publicationTimer); clearInterval(feedbackTimer);
    await scheduler?.close();
    live?.close(); terminals?.close();
    await runtime?.close();
    await jira?.close(); await jiraDelivery?.close(); await publication.close();
    await vite?.close();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    store.close();
    await controller.close();
  } };
}
