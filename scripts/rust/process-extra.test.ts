/** Additional native boundary and cross-runtime contracts, using disposable local processes. */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import * as native from './process-adapter';
import * as retained from '../../server/runtime/processes';
const root = () => realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-native-boundary-')));
const pause = (ms = 20) => new Promise(resolve => setTimeout(resolve, ms));
async function live(job: string) { const deadline = Date.now()+5000; while (Date.now()<deadline) { const result = await native.inspectOwned(job); if (result.state === 'live') return result; await pause(); } throw new Error('No verified live owner'); }

test('native process manifest bytes and pure refusal identities match TypeScript', async t => {
  const directory = root(); const authority = await native.acquireController(join(directory, 'controller'));
  t.after(async () => { await authority.close(); rmSync(directory, { recursive: true, force: true }); });
  const token = randomUUID(); const job = join(directory, 'job');
  const args = ['-c', 'printf "é\\n"', '☃'];
  for (const env of [{}, { PATH: '/bin:/usr/bin', HOME: directory, LANG: 'C' }, { GIT_SSH_COMMAND: 'ssh -o BatchMode=yes', SSH_AUTH_SOCK: '/fixture/socket', GIT_CONFIG_NOSYSTEM: '1', GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_GLOBAL: '/dev/null' }]) {
    const options = { cwd: directory, env };
    assert.deepEqual(native.describeCheckedOwned(job, token, '/bin/sh', args, options, authority), retained.describeCheckedOwned(job, token, '/bin/sh', args, options, authority));
    assert.equal(existsSync(job), false);
  }
  const bad = [
    { token: 'bad' }, { token: token.replaceAll('-', '') }, { command: 'sh' },
    { cwd: directory+'/' }, { cwd: directory+'/.' }, { cwd: '/no-such-directory' },
    { env: { SECRET: 'fixture-only' } }, { root: join(directory,'missing','job') },
  ];
  for (const change of bad) {
    const spec = { root: job, token, command: '/bin/sh', args, cwd: directory, env: {}, ...change };
    for (const backend of [native, retained]) assert.throws(() => backend.describeCheckedOwned(spec.root, spec.token, spec.command, spec.args, { cwd: spec.cwd, env: spec.env }, authority));
    assert.equal(existsSync(job), false);
  }
  const options = { cwd: directory, env: { PATH: '/bin:/usr/bin', HOME: directory } };
  const a = native.prepareCheckedOwned(job, token, '/bin/sh', args, options, authority);
  const b = retained.prepareCheckedOwned(join(directory,'retained'), token, '/bin/sh', args, options, authority);
  assert.equal(a.specSha256, b.specSha256);
  assert.deepEqual(readFileSync(join(a.root,'launch.json')),readFileSync(join(b.root,'launch.json')));
});

test('Rust and retained controllers exclude one another and execute each other’s prepared manifests', async t => {
  const directory = root(); let current: retained.ControllerAuthority | undefined;
  t.after(async () => { await current?.close(); rmSync(directory, { recursive:true, force:true }); });
  for (const [controller, launcher] of [[native, retained], [retained, native]] as const) {
    current = await controller.acquireController(join(directory,'controller'));
    await assert.rejects(launcher.acquireController(join(directory,'controller')), /Another controller/);
    const job = join(directory,randomUUID());
    const descriptor = controller.prepareCheckedOwned(job,randomUUID(),'/bin/sh',['-c','printf cross-runtime'],{cwd:directory,env:{}},current);
    const child = launcher.launchCheckedOwned(descriptor,current);
    assert.deepEqual(await once(child,'exit'),[0,null]);
    assert.equal((await native.inspectCheckedOwned(descriptor)).state,'stopped');
    assert.equal((await retained.inspectCheckedOwned(descriptor)).state,'stopped');
    assert.equal(readFileSync(join(job,'stdout.log'),'utf8'),'cross-runtime');
    await current.close(); current=undefined;
  }
});

test('cross-runtime seals refuse delayed helpers for both prepared and missing manifests', async t => {
  const directory=root(); const authority=await native.acquireController(join(directory,'controller'));
  t.after(async()=>{await authority.close();rmSync(directory,{recursive:true,force:true});});
  for (const prepared of [false,true]) for (const sealing of [native,retained]) {
    const job=join(directory,randomUUID());const marker=join(directory,randomUUID());
    const args=['-c','echo wrong > "$1"','job',marker]; const options={cwd:directory,env:{}};
    const descriptor=native.describeCheckedOwned(job,randomUUID(),'/bin/sh',args,options,authority);
    if (prepared) native.prepareCheckedOwned(job,descriptor.token,'/bin/sh',args,options,authority);
    assert.equal((await sealing.sealCheckedOwned(descriptor)).state,'stopped');
    for (const [program,prefix] of [[native.helper,[]],['python3',[resolve('server/runtime/process-lease.py')]]] as [string,string[]][]) {
      const child=spawn(program,[...prefix,'checked-exec',job,descriptor.token,descriptor.specSha256,descriptor.epoch,descriptor.controllerRoot],{detached:true,stdio:'ignore'});
      assert.notEqual((await once(child,'exit'))[0],0);
    }
    assert.equal(existsSync(marker),false);assert.equal(existsSync(join(job,'launch.json')),prepared);
  }
});

test('a forged owner with a real foreign PID/start/group cannot authorize a signal', async t => {
  const directory=root();const a=join(directory,'a');const b=join(directory,'b');
  const children=[native.launchOwned(a,'/bin/sleep',['30'],{cwd:directory,env:{}}),native.launchOwned(b,'/bin/sleep',['30'],{cwd:directory,env:{}})];
  let saved:Buffer|undefined;
  t.after(async()=>{if(saved)writeFileSync(join(a,'owner.json'),saved);await native.stopOwned(a);await native.stopOwned(b);for(const child of children)child.stdin.end();rmSync(directory,{recursive:true,force:true});});
  await live(a);await live(b);saved=readFileSync(join(a,'owner.json'));
  const foreign=JSON.parse(readFileSync(join(b,'owner.json'),'utf8'));foreign.token=JSON.parse(saved.toString()).token;
  writeFileSync(join(a,'owner.json'),JSON.stringify(foreign));
  assert.equal((await native.stopOwned(a)).state,'uncertain');
  for(const child of children)assert.doesNotThrow(()=>process.kill(child.pid!,0));
});

test('an escaped inherited lease holder remains uncertain after its original group exits', async t => {
  const directory=root();const job=join(directory,'job');const marker=join(directory,'escaped');let pid=0;
  const python=execFileSync('/usr/bin/which',['python3'],{encoding:'utf8'}).trim();
  const script='import os,sys,time\nfrom pathlib import Path\npid=os.fork()\nif pid: os._exit(0)\nos.setsid()\nPath(sys.argv[1]).write_text(str(os.getpid()))\ntime.sleep(30)';
  const child=native.launchOwned(job,python,['-c',script,marker],{cwd:directory,env:{}});
  t.after(async()=>{if(pid){try{process.kill(pid,'SIGKILL');}catch{}}child.stdin.end();await pause(100);rmSync(directory,{recursive:true,force:true});});
  await once(child,'exit');const deadline=Date.now()+5000;while(!existsSync(marker)&&Date.now()<deadline)await pause();
  pid=Number(readFileSync(marker,'utf8'));const ownership=await native.stopOwned(job);
  assert.equal(ownership.state,'uncertain');assert.ok(ownership.members?.includes(pid));assert.doesNotThrow(()=>process.kill(pid,0));
  process.kill(pid,'SIGKILL');await pause(100);assert.equal((await native.inspectOwned(job)).state,'stopped');pid=0;
});


test('a stopped receipt does not hide failed seal preflight after controller-root movement', async t => {
  const directory=root(); const authority=await native.acquireController(join(directory,'controller'));
  t.after(async()=>{await authority.close();rmSync(directory,{recursive:true,force:true});});
  const descriptor=native.describeCheckedOwned(join(directory,'job'),randomUUID(),'/bin/true',[],{cwd:directory,env:{}},authority);
  assert.equal((await native.sealCheckedOwned(descriptor)).state,'stopped');
  renameSync(authority.root,join(directory,'moved'));
  try {
    assert.equal((await native.inspectCheckedOwned(descriptor)).state,'stopped');
    assert.equal((await native.sealCheckedOwned(descriptor)).state,'uncertain');
    assert.equal((await retained.sealCheckedOwned(descriptor)).state,'uncertain');
  } finally { renameSync(join(directory,'moved'),authority.root); }
});
