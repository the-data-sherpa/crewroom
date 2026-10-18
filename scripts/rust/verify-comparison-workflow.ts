/** Opt-in FOR-14 acceptance through the production comparison service.
 * Live runs use real inference. Explicit scripted profile smokes verify runner
 * contracts separately; neither path patches workspaces or posts results from
 * outside the daemon. Preflight is the default mode. */
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { NativeDaemon } from './daemon-client';
import { assertLiveProfile, assertProfileExecution, auditToolCatalogs } from './for14-tool-audit';
import { canonical, digest, readCasePack } from './for14-case-pack';
import { FinalizationObserver } from './for14-finalization-observer';
import { comparisonSettled } from './for14-stop-state';
import { inspectCheckedOwned, inspectOwned, stopOwned } from './process-adapter';

const addition = 'Before reporting an assignment complete, identify the applicable required checks and the user surface affected by the change. Run the checks on the source you are handing off, inspect their results, and identify the checked revision and any limits in the handoff. Earlier passing evidence does not cover later edits. For a user-visible change, exercise the affected behavior on its actual user surface. Make routine reversible choices within the accepted scope and explain them briefly; request a decision when the requirements conflict, needed authority is missing, or the consequence is outside that scope. If verification cannot run or a required check fails, preserve the result and report the assignment as incomplete.';
type Mode = 'preflight' | 'smoke' | 'matrix' | 'recovery';
const setup = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const mode: Mode = process.argv[3] as Mode ?? 'preflight';
const matrixWaitMs = 5_100_000;
const recoveryObservationMs = 480_000;
const recoveryActiveLimitMs = 900_000;
const inference = setup.inference ?? 'live';
assert.ok(['live', 'scripted'].includes(inference));
assert.ok(inference === 'live' || mode === 'smoke', 'Scripted provider contracts are restricted to profile smoke');
assert.ok(['preflight', 'smoke', 'matrix', 'recovery'].includes(mode));
assert.ok(['codex', 'omp', 'claude-code'].includes(setup.kind));
assert.ok(Number(process.versions.node.split('.')[0]) >= 24, 'Use repository-supported Node 24+');
assert.match(setup.evaluationImage, /^sha256:[a-f0-9]{64}$/);
if (mode === 'recovery') {
  assert.equal(setup.wrapperWallSeconds, 780, 'Recovery requires the bounded wrapper');
  assert.equal(setup.wrapperCleanupSeconds, 120);
}
const root = resolve(setup.root), authority = join(root, 'data'), runtime = resolve(setup.runtimeRoot);
mkdirSync(authority, { recursive: true, mode: 0o700 });
mkdirSync(runtime, { recursive: true, mode: 0o700 });
const report: any = { version: 1, mode, kind: setup.kind, passed: false, inference, executionMode: inference,
  scope: mode === 'preflight' ? 'Read-only admission preflight; no evaluated worker execution' : inference === 'scripted'
    ? 'SCRIPTED provider replies through actual runner, production comparison API, real tools and Rust Mail; no live model inference'
    : 'Actual inference via production comparison API, supervised workers and external Rust Mail; isolated configuration',
  startedAt: new Date().toISOString(), packHash: readCasePack().packHash,
  sourceIdentity: setup.sourceIdentity, binaries: setup.binaries, baseline: setup.baseline,
  limits: { perCellActiveMs: 180000, totalActiveMs: 2880000,
    wrapperWallMs: (setup.wrapperWallSeconds ?? (mode === 'matrix' ? 5400 : mode === 'recovery' ? 780 : 900)) * 1000,
    matrixWaitMs: mode === 'matrix' ? matrixWaitMs : null,
    recoveryObservationMs: mode === 'recovery' ? recoveryObservationMs : null,
    recoveryResumeMs: mode === 'recovery' ? 300000 : null,
    recoveryCleanupAllowanceMs: mode === 'recovery' ? (setup.wrapperCleanupSeconds ?? 120) * 1000 : null,
    recoveryExternalEnvelopeMs: mode === 'recovery' ? 900000 : null,
    recoveryActiveLimitMs: mode === 'recovery' ? recoveryActiveLimitMs : null },
  proposalAdditionHash: digest(addition), checks: [], cleanup: { passed: false, processes: [] } };
const provider = setup.kind === 'omp' ? setup.catalogProfile.provider : setup.kind;
const config: any = { authority, roles: join(root, 'roles.json'), runtime: {
  root: runtime, socket: join(runtime, 'tmux.sock'), mail: setup.mail,
  catalog: [setup.catalogProfile ?? { adapter: setup.kind, provider, id: setup.model,
    ...(setup.kind === 'claude-code' ? { resolvedModel: setup.resolvedModel } : {}),
    api: setup.kind === 'codex' ? 'codex-app-server' : 'claude-code-stream',
    inputs: ['text'], efforts: ['low'], tools: true }],
  policy: { default: { model: provider + '/' + setup.model, effort: 'low' } },
  evaluationImage: setup.evaluationImage, commandEngine: { socket: setup.dockerSocket ?? '/var/run/docker.sock' },
  evaluationMode: inference,
} };
if (setup.kind === 'codex') config.runtime.codex = { binary: setup.codex, authFile: setup.authFile };
if (setup.kind === 'claude-code') config.runtime.claudeCode = { binary: setup.claude, home: setup.nativeHome };
if (setup.kind === 'omp') { config.runtime.ompBinary = setup.omp; config.runtime.models = setup.ompModels; }
let client: NativeDaemon | undefined;
let comparisonId: string | undefined;
let finished = false;
let stopRequested = false;
let recoveryControl: Promise<unknown> | undefined;
function requireRecoveryRunning() {
  if (mode === 'recovery') assert.equal(stopRequested, false, 'Recovery stop requested; no further admission or resume');
}
function persist() {
  writeFileSync(join(root, 'comparison-result.json'), JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
}
function save(name: string, value: unknown) {
  const content = JSON.stringify(value, null, 2) + '\n';
  writeFileSync(join(root, name), content, { mode: 0o600 });
  return { path: join(root, name), sha256: digest(content), bytes: Buffer.byteLength(content) };
}
function retainDaemonLog(instance: NativeDaemon) {
  const name = `daemon-${(report.daemonLogs ??= []).length}.stderr.log`;
  writeFileSync(join(root, name), instance.stderr, { mode: 0o600 });
  report.daemonLogs.push({ path: join(root, name), sha256: digest(instance.stderr), bytes: Buffer.byteLength(instance.stderr), exitCode: instance.proc.exitCode, signal: instance.proc.signalCode });
}
async function read() { return client!.request('/api/comparisons/' + comparisonId); }
async function control(action: string) {
  if (action !== 'cancel') requireRecoveryRunning();
  const pending = client!.request(`/api/comparisons/${comparisonId}/control`, { requestId: randomUUID(), action });
  if (mode !== 'recovery' || action === 'cancel') return pending;
  recoveryControl = pending;
  try { return await pending; }
  finally { if (recoveryControl === pending) recoveryControl = undefined; }
}
async function until<T>(probe: () => Promise<T | false>, timeout: number): Promise<T> {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await probe();
    if (value) return value;
    await sleep(750);
  }
  throw new Error('Bounded acceptance deadline reached; retain this comparison and inspect its ownership');
}
const terminal = (state: string) => ['completed', 'incomplete', 'cancelled'].includes(state);
async function stopComparison() {
  // A previously sent Start/Resume must settle before cleanup's Cancel, so a
  // deadline cannot be followed by an in-flight admission after cancellation.
  if (mode === 'recovery') await recoveryControl?.catch(() => undefined);
  if (!client || !comparisonId || client.proc.exitCode !== null || client.proc.signalCode !== null) return;
  const current = await read();
  if (!terminal(current.state)) {
    await control('cancel');
  }
  const final = await until(async () => {
    const value = await read();
    const workspace = await client!.request('/api/workspace');
    return comparisonSettled(value, workspace.queue) ? value : false;
  }, 90000);
  report.cleanup.comparisonSettled = { state: final.state, runIds: final.cells.filter((cell: any) => cell.runId).map((cell: any) => cell.runId), slot: null };
  report.finalComparison = save('comparison-final.json', final);
  if (mode === 'recovery') {
    assert.ok(final.cells.every((cell: any) => (cell.attempts ?? []).every((attempt: any) => attempt.state === 'admitted' || attempt.ownership === 'stopped')), 'Recovery final active ledger requires stopped ownership');
    assert.ok(Number.isFinite(final.activeMs) && final.activeMs >= 0, 'Recovery final active ledger is missing');
    assert.ok(final.activeMs <= recoveryActiveLimitMs, 'Recovery exceeded its separate 15-minute active bound');
    report.recoveryActiveBound = { activeMs: final.activeMs, limitMs: recoveryActiveLimitMs, stopped: true, snapshot: report.finalComparison };
  }
}
let finishing: Promise<void> | undefined;
function finish() {
  if (mode === 'recovery') stopRequested = true;
  return finishing ??= (async () => {
    try { await stopComparison(); } catch (error) { report.cleanup.error = String(error); report.passed = false; }
    if (client && comparisonId && client.proc.exitCode === null && client.proc.signalCode === null) {
      try {
        const final = await read();
        report.events = [];
        for (const cell of final.cells.filter((cell: any) => cell.runId)) {
          const events = await client.command('events', { target: cell.runId });
          report.events.push(save('events-' + cell.id + '.json', events));
        }
      } catch (error) { report.eventRetentionError = String(error); report.passed = false; }
    }
    if (client) {
      try { await client.close(); } catch (error) { report.cleanup.daemonError = String(error); report.passed = false; }
      report.cleanup.daemonExitCode = client.proc.exitCode;
      report.cleanup.daemonSignal = client.proc.signalCode;
      retainDaemonLog(client);
    }
    for (const entry of readdirSync(runtime)) {
      const directory = join(runtime, entry), path = join(directory, 'host-intent.json');
      if (!existsSync(path)) continue;
      try {
        const descriptor = JSON.parse(readFileSync(path, 'utf8')).descriptor;
        const host = await inspectCheckedOwned(descriptor);
        const launch = JSON.parse(readFileSync(join(directory, 'host.json'), 'utf8'));
        const runnerRoot = join(directory, launch.model?.resolved?.adapter === 'codex' ? 'codex-process' : launch.model?.resolved?.adapter === 'claude-code' ? 'claude-process' : 'omp-process');
        const runner = await inspectOwned(runnerRoot);
        const process = { attemptId: entry, hostState: host.state, runnerState: runner.state };
        report.cleanup.processes.push(process);
        if (host.state !== 'stopped') { report.cleanup.fallback = true; await stopOwned(descriptor.root); }
        if (runner.state !== 'stopped') { report.cleanup.fallback = true; await stopOwned(runnerRoot); }
      } catch (error) { (report.cleanup.errors ??= []).push(String(error)); }
    }
    report.cleanup.passed = !report.cleanup.error && !report.cleanup.daemonError && !report.cleanup.errors?.length
      && !report.cleanup.fallback && (!client || client.proc.exitCode === 0);
    report.passed &&= report.cleanup.passed;
    if (mode === 'recovery') report.passed &&= Boolean(report.recoveryActiveBound);
    report.endedAt = new Date().toISOString();
    report.wallMs = Date.parse(report.endedAt) - Date.parse(report.startedAt);
    if (mode === 'recovery' && report.wallMs > report.limits.recoveryExternalEnvelopeMs) {
      report.cleanup.envelopeError = 'Recovery exceeded its 780-second execution plus 120-second cleanup envelope';
      report.cleanup.passed = false; report.passed = false;
    }
    persist(); finished = true;
    console.log(JSON.stringify({ mode, kind: setup.kind, passed: report.passed, error: report.error,
      comparisonId, result: join(root, 'comparison-result.json'), cleanup: report.cleanup.passed }));
  })();
}
process.once('SIGTERM', () => {
  stopRequested = true;
  const request = join(root, 'termination-request.json');
  report.termination = { cause: 'external-signal', signal: 'SIGTERM' };
  try {
    if (existsSync(request)) {
      const value = JSON.parse(readFileSync(request, 'utf8'));
      if (value && typeof value === 'object' && !Array.isArray(value)) report.termination = value;
      else report.termination.requestReadError = 'Termination request is not an object';
    }
  } catch (error) { report.termination.requestReadError = String(error); }
  report.error = report.termination.cause === 'wrapper-wall-deadline' ? 'Wrapper wall deadline reached' : 'External stop signal (SIGTERM)';
  report.passed = false;
  void finish().then(() => process.exit(1));
});

try {
  persist();
  client = new NativeDaemon(config);
  await client.ready;
  requireRecoveryRunning();
  const before = await client.request('/api/workspace');
  const current = before.roles.find((role: any) => role.id === 'implementer');
  assert.ok(current, 'Effective Implementer role must exist');
  assert.ok(!current.instructions.includes(addition), 'Current instruction already includes the candidate; choose a genuinely changed proposal');
  const draft = { ...current, instructions: current.instructions.trimEnd() + '\n\n' + addition };
  report.baselineRole = { id: current.id, revision: current.revision ?? 1, model: current.model, effort: current.effort,
    instructionsHash: digest(current.instructions), source: setup.baseline?.source };
  const preflightInput = { kind: 'role', targetId: current.id, baseRevision: current.revision ?? 1, draft, requestId: randomUUID() };
  const preflight = await client.request('/api/comparisons/preflight', preflightInput);
  report.preflight = save('preflight.json', preflight);
  assert.equal(preflight.canStart, true, JSON.stringify(preflight.reasons));
  assert.equal(preflight.manifest.mode, inference);
  assert.equal(preflight.summary.workers, 16);
  assert.equal(preflight.manifest.design.perCellActiveMs, 180000);
  assert.equal(preflight.manifest.design.totalActiveMs, 2880000);
  const after = await client.request('/api/workspace');
  assert.deepEqual(after.roles, before.roles);
  assert.deepEqual(after.tasks, before.tasks);
  assert.deepEqual(after.runs, before.runs);
  report.checks.push('Preflight freezes four cases, two repetitions, two variants and exact current instructions without starting processes or modifying catalogs');
  if (mode === 'preflight') {
    assert.equal(readdirSync(runtime).filter(entry => existsSync(join(runtime, entry, 'host-intent.json'))).length, 0);
    report.passed = true;
  } else {
    let profile: any;
    if (mode === 'matrix') {
      assert.ok(setup.toolProfileReceipt, 'Matrix requires a prior live tool/profile receipt');
      profile = JSON.parse(readFileSync(setup.toolProfileReceipt, 'utf8'));
      assertLiveProfile(profile);
      assert.equal(profile.mode, 'smoke'); assert.equal(profile.kind, setup.kind); assert.equal(profile.passed, true);
      assert.deepEqual(profile.binaries, setup.binaries, 'Tool/profile smoke must cover the same binaries');
      assert.equal(profile.packHash, report.packHash, 'Tool/profile smoke must cover this frozen pack');
      assert.equal(profile.execution?.image, setup.evaluationImage);
      assert.ok(profile.toolAudits?.some((audit: any) => audit.passed), 'Matrix requires a passing actual runner catalog audit');
      assertProfileExecution(profile.execution, preflight.manifest.execution);
      report.toolProfileReceipt = { path: setup.toolProfileReceipt, sha256: digest(readFileSync(setup.toolProfileReceipt)) };
    }
    const proposal = await client.request('/api/learning', { requestId: randomUUID(), roleId: current.id,
      revision: current.revision ?? 1, mode: 'teach', change: draft.instructions,
      rationale: 'Evaluate verification freshness, actual user surfaces and routine discretion using the bounded comparison. This is a hypothesis; prior correction evidence does not establish an instruction cause.' });
    report.proposal = { id: proposal.id, status: proposal.status };
    const reviewed = await client.request('/api/comparisons/preflight', { ...preflightInput, requestId: randomUUID(), proposalId: proposal.id });
    assert.equal(reviewed.canStart, true, JSON.stringify(reviewed.reasons));
    if (profile) assertProfileExecution(profile.execution, reviewed.manifest.execution);
    report.execution = reviewed.manifest.execution;
    const input = { requestId: randomUUID(), manifest: reviewed.manifest };
    requireRecoveryRunning();
    const created = await client.request('/api/comparisons', input);
    comparisonId = created.id;
    assert.deepEqual(await client.request('/api/comparisons', input), created, 'Creation reply loss replays without another process');
    assert.equal(created.cells.length, 16);
    report.manifestHash = created.manifestHash;
    report.created = save('comparison-created.json', created);
    persist();
    await control('start');
    let lastProgress = 0;
    const finalizationObserver = new FinalizationObserver();
    const snapshot = async () => {
      while (true) {
        const value = await read();
        assert.equal(value.manifestHash, report.manifestHash);
        report.toolAudits ??= [];
        for (const cell of value.cells.filter((cell: any) => cell.result?.observed && !report.toolAudits.some((audit: any) => audit.cellId === cell.id))) {
          report.toolAudits.push(auditToolCatalogs(cell, created.manifest, runtime));
        }
        if (Date.now() - lastProgress >= 15000) {
          lastProgress = Date.now();
          console.log(JSON.stringify({ mode, comparisonId, state: value.state, coverage: value.coverage, activeMs: value.activeMs }));
          report.latest = { state: value.state, coverage: value.coverage, activeMs: value.activeMs }; persist();
        }
        if (value.state === 'held' && !report.held) report.held = save('comparison-held.json', value);
        let observation;
        try { observation = finalizationObserver.observe(value, Date.now()); }
        catch (error) { report.finalizationObservationFailure = save('finalization-observation-failure.json', value); throw error; }
        if (observation.state !== 'none') {
          report.finalizationObservation = { ...observation,
            firstHeld: report.held, scope: 'Read-only observation of accepted handoff assessment; no resume, retry or intervention' };
          persist();
        }
        const unavailable = value.state === 'paused' ? value.cells.find((cell: any) => cell.result?.state === 'unavailable') : undefined;
        if (unavailable) {
          report.unavailable = save('comparison-unavailable.json', value);
          throw new Error('Comparison paused after unavailable cell ' + unavailable.id + ': ' + (unavailable.result.reason ?? unavailable.error ?? 'Inspect retained cell error and ownership'));
        }
        if (observation.state === 'waiting') { await sleep(750); continue; }
        return value;
      }
    };
    if (mode === 'recovery') {
      const meaningfulActivity = (cell: any) => cell.events.find((event: any) =>
        (event.type === 'edit' || event.type === 'check' && !event.payload?.prior)
        && cell.attempts?.some((attempt: any) => attempt.id === event.attemptId && attempt.ownership !== 'stopped'));
      const active = await until(async () => {
        requireRecoveryRunning();
        const value = await snapshot();
        return value.cells.some((cell: any) => cell.result?.observed)
          && value.cells.some((cell: any) => !cell.result && meaningfulActivity(cell)) ? value : false;
      }, recoveryObservationMs);
      const target = active.cells.find((cell: any) => !cell.result && meaningfulActivity(cell));
      const activity = meaningfulActivity(target);
      const originalWorker = target.attempts.find((attempt: any) => attempt.id === activity.attemptId);
      report.recoveryTarget = { cellId: target.id, runId: target.runId, activityEventId: activity.id,
        workerAttemptId: originalWorker.id, workerAssignmentId: originalWorker.assignmentId,
        observed: save('recovery-active.json', active) };
      await control('pause');
      const stopped = await until(async () => { const value = await snapshot(); return value.state === 'paused' ? value : false; }, 90000);
      report.beforeRestart = save('recovery-paused.json', stopped);
      const pausedTarget = stopped.cells.find((cell: any) => cell.id === target.id);
      assert.ok(pausedTarget && !pausedTarget.result, 'Pause raced target completion; this is not the intended active-cell recovery seam');
      assert.ok(stopped.cells.every((cell: any) => (cell.attempts ?? []).every((attempt: any) => attempt.state === 'admitted' || attempt.ownership === 'stopped')));
      assert.ok(pausedTarget.events.some((event: any) => event.id === activity.id));
      report.recoveryTarget.paused = { activeMs: pausedTarget.activeMs, eventIds: pausedTarget.events.map((event: any) => event.id),
        attemptIds: pausedTarget.attempts.map((attempt: any) => attempt.id),
        remainingCellActiveMs: created.manifest.design.perCellActiveMs - pausedTarget.activeMs,
        remainingComparisonActiveMs: created.manifest.design.totalActiveMs - stopped.activeMs };
      const completed = stopped.cells.filter((cell: any) => cell.result).map((cell: any) => ({ id: cell.id, resultHash: cell.resultHash }));
      assert.ok(completed.length > 0, 'Recovery must preserve an actual finished cell');
      requireRecoveryRunning();
      client.proc.kill('SIGKILL'); await client.exit;
      retainDaemonLog(client);
      requireRecoveryRunning();
      client = new NativeDaemon(config); await client.ready;
      requireRecoveryRunning();
      const reopened = await read();
      report.reopened = save('recovery-reopened.json', reopened);
      assert.equal(reopened.state, 'paused');
      assert.equal(reopened.manifestHash, stopped.manifestHash);
      assert.equal(reopened.activeMs, stopped.activeMs);
      const reopenedTarget = reopened.cells.find((cell: any) => cell.id === target.id);
      assert.ok(reopenedTarget && !reopenedTarget.result);
      assert.equal(reopenedTarget.activeMs, pausedTarget.activeMs);
      assert.deepEqual(reopenedTarget.events, pausedTarget.events);
      assert.deepEqual(reopenedTarget.attempts, pausedTarget.attempts);
      assert.deepEqual(reopened.cells.filter((cell: any) => cell.result).map((cell: any) => ({ id: cell.id, resultHash: cell.resultHash })), completed);
      await control('resume');
      const resumed = await until(async () => {
        requireRecoveryRunning();
        const value = await snapshot();
        return value.cells.find((cell: any) => cell.id === target.id)?.result ? value : false;
      }, 300000);
      requireRecoveryRunning();
      const resumedTarget = resumed.cells.find((cell: any) => cell.id === target.id);
      const resumedWorkers = resumedTarget.result.attempts.filter((attempt: any) => attempt.assignmentId === originalWorker.assignmentId
        && !pausedTarget.attempts.some((prior: any) => prior.id === attempt.id));
      assert.ok(resumedTarget.result.observed, 'Same-cell recovery must retain an observed result');
      assert.ok(resumedTarget.activeMs >= pausedTarget.activeMs);
      assert.ok(resumedWorkers.some((attempt: any) => attempt.generation > originalWorker.generation
        && attempt.startedAt && attempt.hostPid && attempt.runnerPid
        && attempt.hostPid !== originalWorker.hostPid && attempt.runnerPid !== originalWorker.runnerPid
        && resumedTarget.events.some((event: any) => event.attemptId === attempt.id && ['edit', 'check', 'handoff'].includes(event.type))),
      'Recovery requires genuine tool activity from a new owned worker generation on the same cell');
      report.recovery = { pausedAfterActivity: active.cells.some((cell: any) => cell.events.length), finishedPreserved: completed,
        resumedCellId: target.id, resumedWorkerAttemptIds: resumedWorkers.map((attempt: any) => attempt.id),
        resumedResult: save('recovery-resumed.json', resumed), unchangedManifest: resumed.manifestHash === created.manifestHash };
      for (const prior of completed) assert.equal(resumed.cells.find((cell: any) => cell.id === prior.id).resultHash, prior.resultHash);
      requireRecoveryRunning();
      report.passed = true;
    } else if (mode === 'smoke') {
      const value = await until(async () => { const value = await snapshot(); return value.cells.some((cell: any) => cell.result?.observed) ? value : terminal(value.state) ? (() => { throw new Error('No live observed cell was produced'); })() : false; }, 360000);
      const cell = value.cells.find((cell: any) => cell.result?.observed);
      const assignments = cell.result.assignments.filter((assignment: any) => !assignment.retiredAt);
      assert.equal(assignments.length, 2);
      assert.equal(assignments.filter((assignment: any) => !assignment.coordinator).length, 1);
      assert.ok(cell.result.attempts.every((attempt: any) => attempt.state === 'admitted' || attempt.ownership === 'stopped'));
      assert.ok(cell.events.some((event: any) => event.type === 'handoff'));
      assert.ok(cell.events.some((event: any) => event.type === 'check' && !event.payload?.prior), 'Profile smoke must exercise a real worker check');
      assert.ok(cell.result.checks.some((check: any) => check.started && check.completed && check.outputValid === true), 'Profile smoke requires a real bound check result');
      const actual = cell.result.attempts.filter((attempt: any) => attempt.hostPid);
      assert.equal(new Set(actual.map((attempt: any) => attempt.hostPid)).size, actual.length);
      assert.ok(actual.length >= 2);
      assert.ok(actual.every((attempt: any) => (attempt.runnerKind ?? 'omp') === setup.kind));
      report.smoke = save('smoke-cell.json', cell);
      report.passed = true;
    } else {
      const value = await until(async () => { const value = await snapshot(); return terminal(value.state) ? value : false; }, matrixWaitMs);
      report.matrix = save('matrix.json', value);
      assert.equal(value.cells.length, 16);
      assert.equal(value.coverage.observed, 16, 'Unavailable or unexecuted cells cannot supply live acceptance');
      const workers = value.cells.flatMap((cell: any) => cell.result.assignments.filter((assignment: any) => !assignment.coordinator).map((assignment: any) => assignment.id));
      assert.equal(new Set(workers).size, 16, 'Replacements are attempts, not independent workers');
      assert.ok(value.cells.every((cell: any) => cell.result.cleanup.state === 'stopped'));
      assert.ok(value.cells.every((cell: any) => cell.result.scores.sourceCorrectness !== null), 'Unavailable oracle results keep acceptance inconclusive');
      report.counts = readCasePack().cases.map(item => ({ caseId: item.id, variants: ['current', 'proposed'].map(variant => {
        const cells = value.cells.filter((cell: any) => cell.caseId === item.id && cell.variant === variant);
        return { variant, denominator: 2, observed: cells.filter((cell: any) => cell.result.observed).length,
          successful: cells.filter((cell: any) => cell.result.scores.successful).length,
          falseCompletion: cells.filter((cell: any) => cell.result.scores.falseCompletion === true).length,
          unknownClaims: cells.filter((cell: any) => cell.result.scores.completionClaim === null).length };
      }) }));
      report.evaluatedWorkers = new Set(workers).size;
      report.attemptCount = value.cells.reduce((total: number, cell: any) => total + cell.result.attempts.length, 0);
      report.conclusion = 'Descriptive four-case observations only; no statistical superiority, equivalence or general operator-burden claim.';
      report.passed = true;
    }
    await stopComparison();
    const unchanged = await client.request('/api/workspace');
    assert.equal(canonical(unchanged.roles), canonical(before.roles), 'Comparison must never adopt instructions');
  }
} catch (error) {
  const detail = error instanceof Error ? error.stack : String(error);
  if (report.termination) report.concurrentShutdownError = detail;
  else report.error = detail;
  report.passed = false;
} finally { if (!finished) await finish(); }
if (!report.passed) process.exitCode = 1;
