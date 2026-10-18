import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { z } from 'zod';
import { startServer } from '../server/app';
import { createRuntimeObserver } from './runtime-observer';

// R7: two runs execute at once under one supervisor, with allocated host ports; a second
// coding run is refused by the explicit one-coding-ticket policy without admitting agents.
const config = z.looseObject({ serverPort: z.number(), root: z.string(), rolePath: z.string(), runtime: z.looseObject({ hostPorts: z.array(z.number()) }) }).parse(JSON.parse(readFileSync(process.argv[2], 'utf8')));
// The harness writes the runtime section in RuntimeConfig shape; the server validates what it uses.
const app = await startServer({ port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime as unknown as Parameters<typeof startServer>[0]['runtimeConfig'] });
const identified = z.looseObject({ id: z.string() });
const runtimeSchema = z.looseObject({
  assignments: z.array(z.unknown()), error: z.string().optional(),
  attempts: z.array(z.looseObject({ hostUrl: z.string().optional(), ompPid: z.number().optional() })),
  messages: z.array(z.looseObject({ disposition: z.string() })),
});
const call = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  return { status: response.status, body: z.record(z.string(), z.unknown()).parse(await response.json()) };
};
const request = async (path: string, body?: unknown) => {
  const result = await call(path, body); if (result.status >= 400) throw new Error(JSON.stringify(result.body)); return result.body;
};
const observe = createRuntimeObserver(request);
try {
  const checkout = join(config.root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { stdio: 'pipe' });
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer.txt'), 'original'); git('add', '.'); git('commit', '-m', 'fixture');
  const repository = identified.parse(await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' }));
  const queue = async (title: string) => {
    const task = identified.parse(await request('/tasks', { title, description: '', repositoryId: repository.id }));
    await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Exchange one peer message', criteria: ['Reply returns to coordinator'] });
    return identified.parse(await request(`/tasks/${task.id}/runs`, { revision: 2 })).id;
  };
  const coding = await queue('Coding run');
  const research = await queue('Concurrent research run');
  const blocked = await queue('Second coding run');
  await request(`/runs/${coding}/repository`, { mode: 'new' });
  await request(`/runs/${blocked}/repository`, { mode: 'new' });
  const team = (coordinator: string, specialist: string) => ({ team: [
    { name: coordinator, roleId: 'triage', coordinator: true, input: `Ask ${specialist} for a confirmation, then receive the reply.` },
    { name: specialist, roleId: 'implementer', coordinator: false, input: `Reply to ${coordinator}.` },
  ] });
  await Promise.all([request(`/runs/${coding}/start`, team('Coordinator', 'SpecialistOne')), request(`/runs/${research}/start`, team('CoordinatorB', 'SpecialistB'))]);
  const refused = await call(`/runs/${blocked}/start`, team('CoordinatorC', 'SpecialistC'));
  assert.equal(refused.status, 409, JSON.stringify(refused.body));
  assert.match(String(refused.body.error), /one coding ticket/);
  const done = async (runId: string, marker: string) => {
    const deadline = Date.now() + 40000; let state;
    while (Date.now() < deadline) {
      state = await observe(runId);
      if (JSON.stringify(state.events).includes(marker)) return runtimeSchema.parse(state);
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.fail(JSON.stringify({ marker, state }));
  };
  const [first, second] = await Promise.all([done(coding, 'A_COMPLETE'), done(research, 'B_COMPLETE')]);
  const attempts = [...first.attempts, ...second.attempts];
  const ports = attempts.map(attempt => Number(new URL(attempt.hostUrl ?? '').port));
  assert.equal(attempts.length, 4);
  assert.equal(new Set(ports).size, 4, `Host ports are allocated per attempt: ${ports}`);
  assert.ok(ports.every(port => config.runtime.hostPorts.includes(port)), 'Pooled ports stay inside the sandbox allowance.');
  assert.equal(new Set(attempts.map(attempt => attempt.ompPid)).size, 4);
  for (const state of [first, second]) {
    assert.equal(state.messages.length, 2);
    assert.ok(state.messages.every(message => message.disposition === 'received'));
  }
  assert.equal(runtimeSchema.parse(await request(`/runs/${blocked}/runtime`)).assignments.length, 0, 'A refused coding run admits no agents.');
  const runs = z.looseObject({ runs: z.array(z.looseObject({ id: z.string(), status: z.string() })) }).parse(await request('/workspace')).runs;
  const status = (id: string) => runs.find(run => run.id === id)?.status;
  assert.equal(status(coding), 'running'); assert.equal(status(research), 'running'); assert.equal(status(blocked), 'awaiting_runner');
  console.log(JSON.stringify({ passed: true, ports, runs: [coding, research], refused: refused.body.error }));
} finally { await app.close(); }
