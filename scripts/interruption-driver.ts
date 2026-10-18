import { fixtureRepository } from './fixture-repository';
import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs';
import { execFileSync, spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { startServer } from '../server/app';
import assert from 'node:assert/strict';
import { ContainerCommand } from '../server/runtime/container-command';
import { request as httpRequest } from 'node:http';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
async function startChild() {
  const runtimePath = config.root + '/runtime-config.json'; writeFileSync(runtimePath, JSON.stringify(config.runtime), { mode: 0o600 });
  const child = spawn(process.execPath, ['--import', 'tsx', resolve('server/index.ts'), '--production'], { stdio: 'pipe', env: { ...process.env,
    CREWROOM_DATA_DIR: config.root, CREWROOM_ROLE_PATH: config.rolePath, CREWROOM_PORT: String(config.serverPort), CREWROOM_RUNTIME_CONFIG: runtimePath } });
  let output = ''; child.stdout.on('data', chunk => output += String(chunk)); child.stderr.on('data', chunk => output += String(chunk));
  const deadline = Date.now() + 15000;
  while (!output.includes('Crewroom listening') && Date.now() < deadline && child.exitCode === null) await new Promise(resolve => setTimeout(resolve, 30));
  assert.ok(output.includes('Crewroom listening'), output);
  return { origin: `http://127.0.0.1:${config.serverPort}`, child, async close() { if (child.exitCode === null && child.signalCode === null) { const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM'); await exited; } } };
}
let backend = config.fault.startsWith('supervisor') ? await startChild() : undefined;
let app = backend ?? await startServer({ port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime });
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
};
const observedRuntime = createRuntimeObserver(request);
try {
  const repository = await request('/repositories', { name: 'Fixture', path: fixtureRepository(config.root), targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Interrupt a child', description: '', repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Run a disposable tool', criteria: ['Pause really stops it'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2, ...(config.fault === 'budget' ? { activeLimitMs: 1000 } : {}) });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Run the slow command.' }] });
  const deadline = Date.now() + 10000; let state: any;
  while (Date.now() < deadline) {
    state = await observedRuntime(run.id);
    if (state.events.some((item: any) => item.event.type === 'tool_started')) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.ok(state.events.some((item: any) => item.event.type === 'tool_started'), 'The real tool must start before pause.');
  // tool_started confirms the container launch, not that its (possibly detached) child
  // has produced output yet. Interrupt only after observable progress exists.
  const ticking = Date.now() + 10000;
  while (!existsSync(config.root + '/workspace/ticks') && Date.now() < ticking) await new Promise(resolve => setTimeout(resolve, 30));
  assert.ok(existsSync(config.root + '/workspace/ticks'), 'The real tool must make progress before interruption.');
  if (config.fault === 'complete') {
    const by = Date.now() + 10000;
    do {
      state = await observedRuntime(run.id);
      if (state.attempts[0].state === 'idle' && JSON.stringify(state.events).includes('Command completed.')) break;
      await new Promise(resolve => setTimeout(resolve, 30));
    } while (Date.now() < by);
    assert.equal(state.attempts[0].state, 'idle');
    assert.ok(JSON.stringify(state.events).includes('Command completed.'));
    assert.equal((await request('/workspace')).runs[0].status, 'running');
    const parent = `${config.runtime.root}/${state.attempts[0].id}/tools`;
    const saved = JSON.parse(readFileSync(`${parent}/${readdirSync(parent)[0]}/container.json`, 'utf8'));
    assert.equal(saved.removed, true, 'Normal completion removes the container before pause.');
    assert.equal(saved.receipt.completed, true);
    assert.ok(saved.receipt.output.includes('finished'));
  }
  if (config.fault.startsWith('supervisor')) {
    if (config.fault === 'supervisor-host') process.kill(state.attempts[0].hostPid, 'SIGKILL');
    const exited = new Promise(resolve => backend!.child.once('exit', resolve)); backend!.child.kill('SIGKILL'); await exited;
    if (config.fault !== 'supervisor-host') assert.doesNotThrow(() => process.kill(state.attempts[0].ompPid, 0));
    backend = await startChild(); app = backend;
    const workspace = await request('/workspace');
    assert.ok(workspace.activity.every((activity: any) => activity.state !== 'ownership_unconfirmed'), 'Restart must reconcile existing process ownership.');
    if (config.fault === 'supervisor-host') assert.equal(workspace.runs[0].status, 'running');
    const recovered = await request(`/runs/${run.id}/runtime`);
    if (config.fault === 'supervisor-host') {
      assert.equal(recovered.attempts.length, 2);
      assert.equal(recovered.attempts[0].ownership, 'stopped');
      assert.equal(recovered.assignments[0].automaticCycles, 1);
      assert.notEqual(recovered.attempts[1].ompPid, state.attempts[0].ompPid);
      const continuedBy = Date.now() + 10000; let completed;
      do { completed = await observedRuntime(run.id); await new Promise(resolve => setTimeout(resolve, 30)); }
      while (!JSON.stringify(completed.events).includes('Recovered retained command state.') && Date.now() < continuedBy);
      assert.ok(JSON.stringify(completed.events).includes('Recovered retained command state.'));
    } else assert.deepEqual(recovered.attempts.map((attempt: any) => attempt.ompPid), state.attempts.map((attempt: any) => attempt.ompPid));
  }
  if (config.fault === 'host') process.kill(state.attempts[0].hostPid, 'SIGKILL');
  if (config.fault === 'tmux') execFileSync('tmux', ['-S', config.runtime.socket, 'kill-server']);
  if (config.fault === 'budget') {
    const deadline = Date.now() + 8000; let observed;
    do { observed = (await request('/workspace')).runs[0]; if (observed.status === 'paused') break; await new Promise(resolve => setTimeout(resolve, 40)); } while (Date.now() < deadline);
    assert.equal(observed.status, 'paused', 'The configured active-time limit must stop execution.');
    assert.equal(observed.pauseReason, 'budget');
    assert.ok(observed.activeMs >= 1000);
  } else await request(`/runs/${run.id}/${config.fault === 'cancel' ? 'cancel' : 'pause'}`, {});
  assert.equal((await request('/workspace')).runs[0].status, config.fault === 'cancel' ? 'cancelled' : 'paused');
  const ticks = readFileSync(config.root + '/workspace/ticks', 'utf8');
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal(readFileSync(config.root + '/workspace/ticks', 'utf8'), ticks);
  state = await request(`/runs/${run.id}/runtime`);
  assert.ok(state.attempts.every((attempt: any) => attempt.state === 'exited'));
  // This assertion runs before fixture cleanup: production owns removal and durable output.
  let removedCommands = 0;
  for (const attempt of state.attempts) {
    const parent = `${config.runtime.root}/${attempt.id}/tools`;
    if (!existsSync(parent)) continue;
    for (const id of readdirSync(parent)) {
      const saved = JSON.parse(readFileSync(`${parent}/${id}/container.json`, 'utf8'));
      assert.equal(saved.removed, true, 'Confirmed stop must remove the exact owned container.');
      assert.ok(saved.receipt, 'Removal must preserve the command receipt.');
      assert.equal(saved.receipt.completed, config.fault === 'complete');
      const status = await new Promise<number | undefined>((resolve, reject) => {
        const request = httpRequest({ socketPath: config.runtime.commandEngine.socket, path: `/containers/${saved.id}/json` }, response => { response.resume(); resolve(response.statusCode); });
        request.once('error', reject); request.end();
      });
      assert.equal(status, 404, 'The exact owned container must be absent from the engine.');
      const job = new ContainerCommand(`${parent}/${id}`, config.runtime.commandEngine);
      assert.deepEqual(await job.receipt(), saved.receipt);
      assert.equal(await job.logs(), saved.receipt.output);
      removedCommands++;
    }
  }
  assert.ok(removedCommands > 0);
  if(config.fault==='cancel') {
    const attempts=state.attempts.length;
    const resumed=await fetch(app.origin+`/api/runs/${run.id}/resume`,{method:'POST',headers:{Origin:app.origin,'Content-Type':'application/json'},body:'{}'});assert.equal(resumed.status,409);
    await app.close();app=await startServer({port:config.serverPort,dataDir:config.root,serveUi:false,rolePath:config.rolePath,runtimeConfig:config.runtime});
    assert.equal((await request('/workspace')).runs[0].status,'cancelled');
    const reopened=await request(`/runs/${run.id}/runtime`);assert.equal(reopened.attempts.length,attempts);assert.ok(reopened.attempts.every((attempt:any)=>attempt.ownership==='stopped'));
    assert.equal(readFileSync(config.root+'/workspace/ticks','utf8'),ticks);
    const commands=[];
    for(const attempt of readdirSync(config.runtime.root)){
      const parent=`${config.runtime.root}/${attempt}/tools`;if(!existsSync(parent))continue;
      for(const id of readdirSync(parent)){const job=new ContainerCommand(parent+'/'+id,config.runtime.commandEngine);const receipt=await job.receipt();assert.equal(receipt.completed,false);assert.ok(receipt.output.length>0 || ticks.length>0);commands.push(receipt);}
    }
    assert.equal(commands.length,1);
  }
  console.log(JSON.stringify({ passed: true, fault: config.fault, checks: ['tool started through real OMP', 'control stops OMP and tool descendants', 'no progress after confirmed stop', ...(config.fault==='cancel'?['cancel terminal across restart','retained interrupted receipt and ticks']:[])], attempts: state.attempts }));
} finally {
  await app.close();
  if (existsSync(config.runtime.root)) for (const attempt of readdirSync(config.runtime.root)) {
    const parent = `${config.runtime.root}/${attempt}/tools`;
    if (!existsSync(parent)) continue;
    for (const id of readdirSync(parent)) {
      const job = new ContainerCommand(parent + '/' + id, config.runtime.commandEngine);
      await job.stop(); await job.remove();
    }
  }
}
