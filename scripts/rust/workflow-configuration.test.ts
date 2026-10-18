/** Actual native host consumes the generated private profile. Assignments and
 * model responses are fixtures; this is not full workflow or live delivery. */
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdirSync,readFileSync,writeFileSync,statSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {homedir} from 'node:os';
import test from 'node:test';
import {supervisorFixture,until} from './supervisor-support';

function profile(f:Awaited<ReturnType<typeof supervisorFixture>>,run:any,assignment:any,attempt:any,previous?:any){
 const root=join(f.runtime,attempt.id);mkdirSync(root,{mode:0o700});writeFileSync(join(root,'fixture-settings.json'),'{}',{mode:0o600});
 const config={root:f.runtime,socket:join(f.runtime,'tmux.sock'),ompBinary:'/fixture/omp',
  launchPrefix:['/usr/bin/env','python3','-u',resolve('scripts/rust/host-fixture-rpc.py')],
  mail:{url:f.b.origin+'/mcp/',token:'synthetic-mail-private',projectRoot:join(f.root,'mail'),retentionSeconds:86400,atcDisabled:true},
  catalog:[assignment.model.resolved],policy:{},models:{providers:{fixture:{baseUrl:f.b.origin+'/v1',
   api:'openai-responses',apiKey:'synthetic-upstream-private',headers:{Authorization:'synthetic-provider-header'},
   models:[{id:'model',contextWindow:1050000,maxTokens:128000}]}}}};
 const op={op:'host',config,run,assignment,attempt,team:[assignment],previous,resumeSession:!!previous,
  providerToken:'synthetic-local-gateway',environment:{HOME:homedir(),PATH:process.env.PATH},publication:false,jira:false};
 const child=spawnSync('target/debug/examples/workflow-inputs',[],{input:JSON.stringify([op]),encoding:'utf8',timeout:15000});
 assert.equal(child.status,0,child.stderr);const result=JSON.parse(child.stdout)[0];assert.ok(!result.error,result.error);
 mkdirSync(result.value.runnerCwd,{recursive:true,mode:0o700});return {root,config:result.value};
}
test('generated research profile launches independent native processes with isolated runner secrets and exact tools',{timeout:60000},async t=>{
 const f=await supervisorFixture(t,1);const seed=f.seeds[0];const p=profile(f,seed.run,seed.assignment,seed.attempt);
 const identity=await f.c.command('launch',{target:seed.attempt.id,revision:seed.run.controlRevision,config:p.config});
 assert.notEqual(identity.hostPid,identity.ompPid);await f.observed();
 const saved=JSON.parse(readFileSync(join(p.root,'host.json'),'utf8'));
 assert.notEqual(saved.token,saved.providerToken);
 const modelsRaw=readFileSync(join(p.root,'config/models.yml'),'utf8'),models=JSON.parse(modelsRaw);
 for(const secret of ['synthetic-upstream-private','synthetic-mail-private','synthetic-provider-header',saved.token])assert.ok(!modelsRaw.includes(secret),'Runner model file exposed an upstream/control secret');
 assert.equal(models.providers.fixture.apiKey,'synthetic-local-gateway');assert.deepEqual(models.providers.fixture.headers,{});
 assert.ok(models.providers.fixture.baseUrl.startsWith('http://127.0.0.1:'));
 assert.equal(statSync(join(p.root,'config/models.yml')).mode&0o077,0);
 const requests=readFileSync(join(p.root,'rpc-requests.jsonl'),'utf8').trim().split('\n').map(s=>JSON.parse(s));
 const tools=requests.find((r:any)=>r.type==='set_host_tools').tools.map((r:any)=>r.name);
 assert.ok(tools.includes('submit_research_artifact')&&tools.includes('request_research_completion'));
 for(const name of ['edit_file','write_file','claim_implementation','run_check','publish_change','propose_refinement'])assert.ok(!tools.includes(name),name);
 await f.c.command('activate',{target:seed.run.id});await f.input('prepared-profile',{});
 await until(async()=>(await f.events()).some((e:any)=>e.event.type==='prompt_result'&&e.event.id==='prepared-profile'));
 assert.equal((await f.stop()).stopped,true);
});
test('generated replacement preserves assignment cwd and falls back from an unconfirmed retained session',{timeout:60000},async t=>{
 const f=await supervisorFixture(t,1),seed=f.seeds[0];const first=profile(f,seed.run,seed.assignment,seed.attempt);
 await f.c.command('launch',{target:seed.attempt.id,revision:seed.run.controlRevision,config:first.config});
 await f.c.command('activate',{target:seed.run.id});await f.input('before',{});
 await until(async()=>(await f.events()).some((e:any)=>e.event.type==='prompt_result'&&e.event.id==='before'));
 await f.c.command('pause',{target:seed.run.id});assert.equal((await f.stop()).stopped,true);await f.c.command('finish_pause',{target:seed.run.id});
 const previous=await f.c.command('attempt',{target:seed.attempt.id});
 writeFileSync(join(previous.sessionDirectory,'saved-session'),'scripted different session');
 const replacement=await f.c.command('replace',{target:seed.assignment.id});await f.c.command('continue',{target:seed.run.id});
 const runtime=await f.c.command('runtime',{target:seed.run.id});
 const assignment=runtime.assignments.find((a:any)=>a.id===seed.assignment.id);
 const run=(await f.c.command('snapshot')).runs.find((r:any)=>r.id===seed.run.id);
 const second=profile(f,run,assignment,replacement,previous);
 assert.equal(second.config.runnerCwd,first.config.runnerCwd);assert.equal(second.config.resumeSessionDir,previous.sessionDirectory);
 const identity=await f.c.command('launch',{target:replacement.id,revision:run.controlRevision,config:second.config});
 assert.equal(identity.recoveryMode,'replacement');assert.equal(identity.sessionDirectory,join(second.root,'sessions'));
 assert.equal(identity.sessionId,'fresh-session');
 await until(async()=>(await f.events()).some((e:any)=>e.attemptId===replacement.id&&e.event.type==='session_restore_inspected'));
 assert.equal((await f.stop()).stopped,true);assert.equal((await f.c.command('owned',{target:replacement.id})).state,'stopped');
});
