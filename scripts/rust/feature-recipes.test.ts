/** FOR-12: three selected native journeys, with scripted RPC/Mail and actual
 * daemon, GPUI, Git, owned OS processes, and (readiness only) Docker checks. */
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {existsSync, readFileSync, writeFileSync} from 'node:fs';
import {join} from 'node:path';
import test, {type TestContext} from 'node:test';
import {NativeDaemon} from './daemon-client';
import {nativeUi} from './native-ui-driver';
import {inspectCheckedOwned, inspectOwned} from './process-adapter';
import {reviewedCandidate} from './workflow-coding-fixture';
import {workflowFixture, until, type FixtureCleanup} from './workflow-support';

const nativeSelector = 'workbench::journey_tests::feature_recipes::native_feature_recipe_control';
type Fixture = Awaited<ReturnType<typeof workflowFixture>>;
type Receipt = {
  schemaVersion: 1; recipe: string; outcome: string; startedAt: string; finishedAt?: string;
  fixtureRoot?: string; taskId?: string; runId?: string; error?: string;
  modes: Record<string, string>; assertions: {id: string; passed: true; facts: unknown}[];
  native: any[]; durable: any[]; cleanup?: FixtureCleanup;
};

// Shared artifacts contain synthetic configuration, but published observations
// still exclude credentials and complete launch descriptors by construction.
function sanitized(value: any): any {
  if (Array.isArray(value)) return value.map(sanitized);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value)
    .filter(([key]) => !/^(token|authorization|apiKey|headers|environment|env)$/i.test(key))
    .map(([key, item]) => [key, sanitized(item)]));
  return value;
}
function jsonl(path: string): any[] {
  return readFileSync(path, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line));
}
function artifact(path: string) {
  const body = readFileSync(path);
  return {path, bytes: body.length, sha256: createHash('sha256').update(body).digest('hex')};
}
function report(recipe: string) {
  const data: Receipt = {schemaVersion: 1, recipe, outcome: 'incomplete', startedAt: new Date().toISOString(),
    modes: {surface: 'native GPUI test controls against production Rust daemon', runner: 'independent scripted Python RPC processes',
      provider: 'scripted loopback Mail; no live model, Jira, or PR delivery', process: 'actual ownership-checked OS processes'},
    assertions: [], native: [], durable: []};
  const save = () => {
    if (process.env.CREWROOM_FEATURE_RESULT) writeFileSync(process.env.CREWROOM_FEATURE_RESULT,
      JSON.stringify(sanitized(data), null, 2) + '\n', {mode: 0o600});
  };
  save();
  return {data, save,
    pass(id: string, facts: unknown = {}) {data.assertions.push({id, passed: true, facts}); save();},
    snapshot(phase: string, value: unknown) {data.durable.push({phase, value: sanitized(value)}); save();},
    cleanup(value: FixtureCleanup) {data.cleanup = value; if (!value.passed) data.outcome = 'incomplete'; save();},
  };
}
type Report = ReturnType<typeof report>;
async function setup(t: TestContext, r: Report, purpose: 'research' | 'coding') {
  const f = await workflowFixture(t, purpose, 0, false, {service: true,
    parent: process.env.CREWROOM_FEATURE_FIXTURE_PARENT, cleanup: value => r.cleanup(value),
    configure({config}) {if (purpose === 'research') config.runtime.launchPrefix.push('--unique-sessions');},
  });
  assert.ok(f.c instanceof NativeDaemon);
  Object.assign(r.data, {fixtureRoot: f.root, taskId: f.task.id, runId: f.run.id}); r.save();
  return f;
}
async function ui(t: TestContext, f: Fixture, r: Report, action: string, kind: string, extra: Record<string, unknown> = {}) {
  const value = await nativeUi(t, f.root, {descriptor: join(f.config.authority, 'native-connection.json'),
    taskId: f.task.id, runId: f.run.id, action, kind, ...extra}, nativeSelector);
  assert.equal(value.connected, true); assert.equal(value.runtimeFresh, true);
  assert.equal(value.runtime.run.id, f.run.id);
  r.data.native.push(sanitized(value)); r.save();
  return value;
}
async function owned(f: Fixture, attempt: any) {
  const root = join(f.runtime, attempt.id);
  const descriptor = JSON.parse(readFileSync(join(root, 'host-intent.json'), 'utf8')).descriptor;
  const host = await inspectCheckedOwned(descriptor);
  const runner = await inspectOwned(join(root, 'omp-process'));
  // Tokens in the original descriptors remain only in private fixture files.
  return {attemptId: attempt.id, host: sanitized(host), runner: sanitized(runner)};
}
async function execute(t: TestContext, recipe: string, run: (r: Report) => Promise<void>) {
  const r = report(recipe);
  try {await run(r); r.data.outcome = 'passed';}
  catch (error) {r.data.outcome = 'failed'; r.data.error = error instanceof Error ? error.message : String(error); throw error;}
  finally {r.data.finishedAt = new Date().toISOString(); r.save();}
}

test('steering-reconnect', {timeout: 180000}, async t => execute(t, 'steering-reconnect', async r => {
  const f = await setup(t, r, 'research'); await f.start(); const initial = await f.idle();
  const coordinator = initial.assignments.find((a: any) => a.coordinator).currentAttemptId;
  r.snapshot('running-coordinator', initial);
  // Holding an observed Mail call holds the workflow delivery lane. The daemon
  // remains reachable; storing the instruction does not claim offline submission.
  const previousRequests = f.requests.length;
  f.holdMail();
  await until(() => f.requests.slice(previousRequests).some(request => request.name === 'fetch_inbox_events'));
  const body = 'FOR-12: retain this direction across reconnect; receipt is separate from application.';
  const stored = await ui(t, f, r, 'steering-store', 'store-steering', {body, delivery: 'pending'});
  const messages = await f.c.command('steering', {target: f.run.id});
  assert.equal(messages.length, 1); const message = messages[0];
  assert.equal(message.body, body); assert.equal(message.delivery, 'pending');
  assert.equal(stored.messages.filter((m: any) => m.id === message.id).length, 1);
  r.snapshot('stored-pending', messages); r.pass('native-stored-pending', {messageId: message.id});

  // Each native action is a fresh GPUI client process; this is an actual client
  // reconnect to the same authority, not a rewritten or seeded message record.
  const reopened = await ui(t, f, r, 'steering-reconnect-pending', 'inspect-steering', {body, messageId: message.id, delivery: 'pending'});
  assert.equal(reopened.messages.filter((m: any) => m.id === message.id).length, 1);
  assert.deepEqual(await f.c.command('steering', {target: f.run.id}), messages);
  r.pass('reconnect-preserves-pending', {messageId: message.id, nativeClients: 2});
  f.releaseMail();
  const received = await until(async () => {
    const list = await f.c.command('steering', {target: f.run.id});
    return list[0]?.delivery === 'received' ? list[0] : false;
  });
  assert.equal(received.id, message.id);
  const inputId = 'steering:' + message.id;
  const hostReceipts = jsonl(join(f.runtime, coordinator, 'inputs.jsonl')).filter(input => input.id === inputId);
  assert.ok(hostReceipts.some(input => ['received', 'completed'].includes(input.disposition)));
  const wire = jsonl(join(f.runtime, coordinator, 'rpc-requests.jsonl')).filter(frame => frame.type === 'prompt' && frame.id === inputId);
  assert.equal(wire.length, 1); assert.equal(JSON.parse(wire[0].message).steering.id, message.id);
  r.snapshot('host-received', {message: received, hostReceipts, wireInput: JSON.parse(wire[0].message)});
  r.pass('host-received', {messageId: message.id, inputId, exactlyOneRpcPrompt: true});
  await ui(t, f, r, 'steering-received', 'inspect-steering', {body, messageId: message.id, delivery: 'received'});
  r.pass('native-received', {messageId: message.id});
  await f.idle();
  // Exercise the host's authenticated tool relay, not Store.applySteering.
  await f.input(coordinator, {kind: 'tool', name: 'steering_receipt', toolCallId: 'feature-steering-applied',
    arguments: {messageId: message.id, disposition: 'applied', reason: 'Reconciled into this scripted coordinator plan; no completed-work claim.'}}, 'feature-apply');
  const applied = await until(async () => {
    const list = await f.c.command('steering', {target: f.run.id});
    return list[0]?.delivery === 'applied' ? list[0] : false;
  });
  await f.idle();
  const toolReplies = jsonl(join(f.runtime, coordinator, 'rpc-requests.jsonl'))
    .filter(frame => frame.type === 'host_tool_result' && frame.id === 'workflow-tool-feature-apply');
  assert.equal(toolReplies.length, 1); assert.notEqual(toolReplies[0].isError, true);
  assert.equal(applied.id, message.id);
  const transitions = applied.transitions.map((transition: any) => transition.state);
  assert.ok(transitions.includes('received')); assert.equal(transitions.filter((state: string) => state === 'applied').length, 1);
  r.snapshot('host-applied', {message: applied, toolReplies}); r.pass('host-applied', {messageId: message.id, transitions});
  const final = await ui(t, f, r, 'steering-reconnect-applied', 'inspect-steering', {body, messageId: message.id, delivery: 'applied'});
  const finalMessages = await f.c.command('steering', {target: f.run.id});
  assert.equal(finalMessages.length, 1); assert.deepEqual(finalMessages[0], applied);
  assert.deepEqual(final.messages.find((m: any) => m.id === message.id), applied);
  r.snapshot('final-steering', finalMessages); r.pass('reconnect-preserves-applied', {messageId: message.id, applications: 1});
}));

test('pause-resume', {timeout: 180000}, async t => execute(t, 'pause-resume', async r => {
  const f = await setup(t, r, 'research'); await f.start(); const initial = await f.idle();
  const assignment = initial.assignments.find((a: any) => a.coordinator);
  const attempt = initial.attempts.find((a: any) => a.id === assignment.currentAttemptId);
  const progressPath = join(f.runtime, attempt.id, 'rpc-requests.jsonl');
  // The independent RPC process appends this request before entering its existing
  // bounded 20-second work latch. Freeze the verified runner before native Pause
  // so pausing cannot be mistaken for already stopped work.
  await f.input(attempt.id, {gate: 'feature-active-work', events: [{type: 'agent_start'},
    {type: 'message_update', messageId: 'feature-progress', phase: 'text_delta', contentIndex: 0}]}, 'feature-active-progress');
  await until(() => existsSync(join(f.runtime, attempt.id, 'feature-active-work-waiting')));
  await until(async () => (await f.snapshot()).attempts.find((a: any) => a.id === attempt.id)?.state === 'executing');
  const ownership = await owned(f, attempt);
  assert.equal(ownership.host.state, 'live'); assert.equal(ownership.runner.state, 'live');
  assert.equal(ownership.runner.owner.pid, attempt.ompPid);
  assert.ok(jsonl(progressPath).some(frame => frame.type === 'prompt' && frame.id === 'feature-active-progress'));
  const progress = artifact(progressPath);
  r.snapshot('active-progress', {runtime: await f.snapshot(), ownership, progress});
  r.pass('owned-active-progress', {attemptId: attempt.id, progress});
  process.kill(attempt.ompPid, 'SIGSTOP');
  const paused = await ui(t, f, r, 'pause-active', 'pause');
  assert.equal(paused.intermediate.runtime.run.status, 'pausing');
  assert.ok(paused.intermediate.runtime.attempts.some((a: any) => a.ownership !== 'stopped'));
  r.pass('native-pausing-before-stopped', {attemptId: attempt.id});
  const stoppedOwnership = await owned(f, attempt);
  assert.equal(stoppedOwnership.host.state, 'stopped'); assert.equal(stoppedOwnership.runner.state, 'stopped');
  const stopped = await f.snapshot();
  assert.equal(stopped.run.status, 'paused'); assert.ok(stopped.attempts.every((a: any) => a.ownership === 'stopped'));
  const stoppedProgress = artifact(progressPath);
  r.snapshot('paused-owned-stop', {runtime: stopped, ownership: stoppedOwnership, progress: stoppedProgress});
  r.pass('paused-owned-stop', {attemptId: attempt.id, host: 'stopped', runner: 'stopped'});
  const previousDaemon = f.c;
  await previousDaemon.close();
  assert.equal(previousDaemon.proc.exitCode, 0, previousDaemon.stderr);
  assert.equal(previousDaemon.proc.signalCode, null);
  await f.reopen();
  assert.notEqual(f.c.proc.pid, previousDaemon.proc.pid);
  r.snapshot('service-reopen', {previousDaemon: {pid: previousDaemon.proc.pid,
    exitCode: previousDaemon.proc.exitCode, signalCode: previousDaemon.proc.signalCode}, currentDaemon: {pid: f.c.proc.pid}});
  const history = await ui(t, f, r, 'pause-reopen-history', 'history', {attemptId: attempt.id});
  assert.equal(history.runtime.run.status, 'paused'); assert.equal(history.runtime.run.id, f.run.id);
  assert.deepEqual(history.runtime.assignments.map((a: any) => a.id), stopped.assignments.map((a: any) => a.id));
  assert.deepEqual(history.runtime.attempts.map((a: any) => a.id), stopped.attempts.map((a: any) => a.id));
  assert.deepEqual(artifact(progressPath), stoppedProgress);
  r.snapshot('reopened-history', history.runtime); r.pass('reopen-preserves-history', {runId: f.run.id, attemptId: attempt.id, progress: stoppedProgress});
  await ui(t, f, r, 'pause-explicit-resume', 'resume'); const resumed = await f.idle();
  const next = resumed.attempts.find((a: any) => a.id === resumed.assignments.find((a: any) => a.id === assignment.id).currentAttemptId);
  assert.notEqual(next.id, attempt.id); assert.notEqual(next.ompPid, attempt.ompPid); assert.notEqual(next.hostPid, attempt.hostPid);
  assert.equal(resumed.run.id, stopped.run.id); assert.ok(resumed.attempts.some((a: any) => a.id === attempt.id && a.ownership === 'stopped'));
  r.pass('native-explicit-resume', {runId: f.run.id, previousAttempt: attempt.id, replacementAttempt: next.id});
  await f.input(next.id, {events: [{type: 'agent_start'}, {type: 'session_settled'}]}, 'feature-resumed-progress');
  await until(() => jsonl(join(f.runtime, next.id, 'rpc-requests.jsonl')).some(frame => frame.type === 'prompt' && frame.id === 'feature-resumed-progress'));
  await f.idle();
  const oldOwnership = await owned(f, attempt), nextOwnership = await owned(f, next);
  assert.equal(oldOwnership.host.state, 'stopped'); assert.equal(oldOwnership.runner.state, 'stopped');
  assert.equal(nextOwnership.host.state, 'live'); assert.equal(nextOwnership.runner.state, 'live');
  assert.deepEqual(artifact(progressPath), stoppedProgress);
  const currentProgress = artifact(join(f.runtime, next.id, 'rpc-requests.jsonl'));
  r.snapshot('resumed-progress', {runtime: await f.snapshot(), oldOwnership, nextOwnership, progress: currentProgress});
  r.pass('resumed-progress-no-old-writer', {previousAttempt: attempt.id, replacementAttempt: next.id, progress: currentProgress});
}));

test('readiness-invalidation', {timeout: 300000}, async t => execute(t, 'readiness-invalidation', async r => {
  const f = await setup(t, r, 'coding');
  const {candidate, reviewer} = await reviewedCandidate(f); r.data.runId = f.run.id; r.save();
  const before = await f.c.command('record', {target: f.run.id});
  const assessment = await f.c.command('evidence', {target: f.run.id});
  assert.equal(assessment.ready, true); assert.equal(before.checks[0].exitCode, 0); assert.equal(before.checks[0].sourceIntact, true);
  assert.equal(before.reviews[0].verdict, 'pass'); assert.equal(before.reviews[0].candidateId, candidate.id);
  assert.equal(typeof before.reviews[0].attemptId, 'string'); assert.ok(before.reviews[0].attemptId.length > 0);
  assert.equal(before.reviews[0].attemptId, reviewer);
  assert.ok(!candidate.implementationAttempts.includes(before.reviews[0].attemptId));
  r.snapshot('ready-candidate', {record: before, assessment});
  r.pass('ready-candidate-real-check-review', {candidateId: candidate.id, revision: candidate.revision, checkId: before.checks[0].id, reviewId: before.reviews[0].id, reviewerAttemptId: reviewer});
  await f.pause();
  const ready = await ui(t, f, r, 'readiness-positive', 'results', {ready: true});
  assert.equal(ready.result.assessment.ready, true); r.pass('native-ready-results', {candidateId: candidate.id});
  const stopped = await f.snapshot();
  assert.equal(stopped.run.status, 'paused'); assert.ok(stopped.attempts.every((a: any) => a.ownership === 'stopped'));
  const ownership = await Promise.all(stopped.attempts.map((attempt: any) => owned(f, attempt)));
  assert.ok(ownership.every(value => value.host.state === 'stopped' && value.runner.state === 'stopped'));
  r.snapshot('stopped-before-refresh', {runtime: stopped, ownership}); r.pass('stopped-before-refresh');
  const git = (...args: string[]) => execFileSync('git', ['-C', f.source, ...args], {encoding: 'utf8'}).trim();
  const oldTarget = git('rev-parse', 'HEAD');
  writeFileSync(join(f.source, 'target-only.txt'), 'A nonconflicting target revision for FOR-12.\n');
  git('add', 'target-only.txt'); git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'FOR-12 advance isolated target');
  const target = git('rev-parse', 'HEAD'); assert.notEqual(target, oldTarget);
  r.snapshot('advanced-isolated-target', {path: f.source, oldTarget, target});
  const refreshed = await ui(t, f, r, 'readiness-target-refresh', 'target-refresh', {ready: false, historicalCandidateId: candidate.id});
  r.pass('native-target-refresh', {control: 'target-refresh', target});
  const after = await f.c.command('record', {target: f.run.id});
  const invalid = await f.c.command('evidence', {target: f.run.id});
  assert.equal(after.candidates.length, 2); const next = after.candidates.at(-1);
  assert.notEqual(next.id, candidate.id); assert.equal(next.refreshedFrom, candidate.id);
  for (const key of ['head', 'base', 'tree']) assert.notEqual(next.revision[key], candidate.revision[key], key + ' must change');
  assert.equal(next.revision.base, target); assert.equal(invalid.ready, false); assert.ok(invalid.blockers.length > 0);
  assert.equal(refreshed.result.assessment.ready, false);
  r.snapshot('invalidated-assessment', {assessment: invalid, previousRevision: candidate.revision, nextRevision: next.revision});
  r.pass('revision-invalidates-readiness', {previousCandidate: candidate.id, nextCandidate: next.id, blockers: invalid.blockers});
  assert.deepEqual(after.candidates[0], before.candidates[0]); assert.deepEqual(after.checks, before.checks); assert.deepEqual(after.reviews, before.reviews);
  assert.ok(after.checks.every((check: any) => check.candidateId !== next.id)); assert.ok(after.reviews.every((review: any) => review.candidateId !== next.id));
  const final = await f.snapshot(); assert.equal(final.run.status, 'paused'); assert.equal(final.run.desiredState, 'paused');
  assert.deepEqual(final.attempts.map((a: any) => a.id), stopped.attempts.map((a: any) => a.id));
  assert.ok(final.attempts.every((a: any) => a.ownership === 'stopped'));
  r.snapshot('historical-evidence-and-no-restart', {record: after, runtime: final});
  r.pass('historical-evidence-retained', {candidateId: candidate.id, checks: after.checks.map((check: any) => check.id), reviews: after.reviews.map((review: any) => review.id)});
}));
