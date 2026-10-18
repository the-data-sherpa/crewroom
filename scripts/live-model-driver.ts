import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { startServer } from '../server/app';
import { inspectOwned } from '../server/runtime/processes';
import { createRuntimeObserver } from './runtime-observer';

const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: config.serverPort, dataDir: config.root, rolePath: config.rolePath, runtimeConfig: config.runtime, serveUi: false });
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const value = await response.json(); assert.ok(response.ok, JSON.stringify(value)); return value;
};
const observedRuntime = createRuntimeObserver(request);
let runId: string | undefined;
let report: Record<string, unknown>;
const scope = 'Live tool routing and identity only; this is not evidence of coding or review quality. No Jira, Bitbucket or repository mutation.';
try {
  const repository = await request('/repositories', { name: 'Admission probe', path: config.root, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Live model routing probe', description: 'Pass the exact token CREWROOM_MODEL_PROBE through the assigned chain once.', repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: task.description, criteria: ['The coordinator receives the token from Reporter.'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 }); runId = run.id;
  await request(`/runs/${run.id}/budget`, { activeLimitMs: 300000 });
  await request(`/runs/${run.id}/start`, { team: [
    { name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'First send exactly CREWROOM_MODEL_PROBE to Implementer using send_message. Then wait. When Reporter sends the token back, reply only MODEL_PROBE_COMPLETE as final text. Do not send another message.' },
    { name: 'Implementer', roleId: 'implementer', coordinator: false, input: 'On receiving the probe, use send_message exactly once to send CREWROOM_MODEL_PROBE to Reviewer. Then stop.' },
    { name: 'Reviewer', roleId: 'reviewer', coordinator: false, input: 'On receiving the probe, use send_message exactly once to send CREWROOM_MODEL_PROBE to Reporter. Then stop.' },
    { name: 'Reporter', roleId: 'delivery-reporter', coordinator: false, input: 'On receiving the probe, use send_message exactly once to send CREWROOM_MODEL_PROBE to Coordinator. Then stop.' },
  ] });
  const deadline = Date.now() + 240000; let runtime;
  do {
    runtime = await observedRuntime(run.id);
    if (runtime.messages.length === 4 && runtime.messages.every((item: any) => item.disposition === 'received') && runtime.attempts.every((item: any) => item.state === 'idle')) break;
    const current = (await request('/workspace')).runs[0]; if (current.status === 'attention_required') throw new Error(current.attentionReason);
    await new Promise(resolve => setTimeout(resolve, 250));
  } while (Date.now() < deadline);
  assert.equal(runtime.messages.length, 4); assert.ok(runtime.messages.every((item: any) => item.disposition === 'received' && item.body === 'CREWROOM_MODEL_PROBE'));
  assert.equal(new Set(runtime.attempts.map((item: any) => item.ompPid)).size, 4); assert.equal(new Set(runtime.attempts.map((item: any) => item.sessionId)).size, 4);
  assert.ok(JSON.stringify(runtime.events.filter((item: any) => item.event.type === 'message_end' && item.event.message?.role === 'assistant')).includes('MODEL_PROBE_COMPLETE'));
  report = { passed: true, at: new Date().toISOString(), independentProcesses: 4, mailReceipts: 4, models: runtime.assignments.map((item: any) => ({ name: item.name, selector: item.model.resolved.selector, effort: item.model.resolved.effort })),
    providerIdentities: runtime.events.filter((item: any) => item.event.type === 'provider_request').map((item: any) => item.event.identity),
    scope };
} catch (error) {
  const runtime = runId ? await observedRuntime(runId).catch(() => undefined) : undefined;
  report = { passed: false, at: new Date().toISOString(), reason: error instanceof Error ? error.message : 'Probe failed', scope,
    mailReceipts: runtime?.messages.filter((item: any) => item.disposition === 'received').length ?? 0,
    models: runtime?.assignments.map((item: any) => ({ name: item.name, selector: item.model.resolved.selector, effort: item.model.resolved.effort })),
    attempts: runtime?.attempts.map((item: any) => ({ id: item.id, state: item.state, ompPid: item.ompPid, sessionId: item.sessionId })),
    providerIdentities: runtime?.events.filter((item: any) => item.event.type === 'provider_request').map((item: any) => item.event.identity) };
  process.exitCode = 1;
} finally {
  if (runId) await request(`/runs/${runId}/pause`, {}).catch(() => {});
  await app.close();
}
const ownership = existsSync(config.runtime.root) ? await Promise.all(readdirSync(config.runtime.root).filter(id => existsSync(join(config.runtime.root, id, 'host.json'))).map(async id => ({ attemptId: id,
  host: (await inspectOwned(join(config.runtime.root, id, 'host-process'))).state,
  omp: (await inspectOwned(join(config.runtime.root, id, 'omp-process'))).state }))) : [];
report!.ownership = ownership;
if (ownership.some(item => item.host !== 'stopped' || item.omp !== 'stopped')) { report!.passed = false; process.exitCode = 1; }
writeFileSync(join(config.root, 'report.json'), JSON.stringify(report!, null, 2) + '\n'); console.log(JSON.stringify(report!));
