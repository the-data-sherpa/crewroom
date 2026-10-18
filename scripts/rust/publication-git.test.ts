import assert from 'node:assert/strict';
import {test,type TestContext} from 'node:test';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,readdirSync,realpathSync,rmSync,statSync,existsSync,chmodSync,renameSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import {NativePublicationGit} from './publication-git-adapter';
import {inspectCheckedOwned,sealCheckedOwned,stopOwned} from './process-adapter';
import {setTimeout as delay} from 'node:timers/promises';
const git=(cwd:string,...args:string[])=>execFileSync('git',['-C',cwd,...args],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
function fixture(t:TestContext){
 const root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-publication-'))),checkout=join(root,'checkout'),remote=join(root,'remote.git'),jobs=join(root,'jobs');mkdirSync(checkout);
 git(checkout,'init','-b','main');git(checkout,'config','user.name','Fixture');git(checkout,'config','user.email','fixture@example.invalid');git(root,'init','--bare','-b','main',remote);
 writeFileSync(join(checkout,'answer'),'original');git(checkout,'add','.');git(checkout,'commit','-m','initial');const first=git(checkout,'rev-parse','HEAD');git(checkout,'push',remote,'HEAD:refs/heads/main');git(remote,'tag','fixture-tag',first);
 writeFileSync(join(checkout,'answer'),'change');git(checkout,'commit','-am','change');const head=git(checkout,'rev-parse','HEAD');
 const clients:NativePublicationGit[]=[];const expectedCloseErrors=new Set<NativePublicationGit>();const open=(env?:NodeJS.ProcessEnv,helper?:string)=>{const client=new NativePublicationGit(jobs,{fixturePath:remote},['main','master','release'],{env,helper});clients.push(client);return client;};
 t.after(async()=>{for(const client of clients){try{await client.close();}catch(error){if(!expectedCloseErrors.has(client))throw error;}}for(const intent of intents(jobs)){let state=await sealCheckedOwned(intent.descriptor);if(state.state==='live'){await stopOwned(intent.descriptor.root);state=await inspectCheckedOwned(intent.descriptor);}assert.equal(state.state,'stopped',JSON.stringify(state));}rmSync(root,{recursive:true,force:true});});return {root,checkout,remote,jobs,first,head,open,expectedCloseErrors};
}
function intents(root:string){return readdirSync(root).filter(n=>n.endsWith('.json')).map(n=>JSON.parse(readFileSync(join(root,n),'utf8')));}
async function waitFor(check:()=>boolean,ms=10000){const until=Date.now()+ms;while(!check()){if(Date.now()>until)throw new Error('Fixture condition did not arrive');await delay(10);}}
function driver(root:string){
 const real=realpathSync(execFileSync('/usr/bin/which',['git'],{encoding:'utf8'}).trim());const bin=join(root,'driver');mkdirSync(bin);const config=join(bin,'rule.json'),arrived=join(bin,'arrived'),release=join(bin,'release'),stopping=join(bin,'stopping'),calls=join(bin,'calls');writeFileSync(config,'{}');
 writeFileSync(join(bin,'git'),`#!${process.execPath}\nconst fs=require('node:fs'),cp=require('node:child_process');const args=process.argv.slice(2),rule=JSON.parse(fs.readFileSync(${JSON.stringify(config)},'utf8'));fs.appendFileSync(${JSON.stringify(calls)},JSON.stringify(args)+'\\n');const matched=rule.op===args[6];if(rule.ignoreTerm)process.on('SIGTERM',()=>fs.writeFileSync(${JSON.stringify(stopping)},'term'));(async()=>{if(matched&&rule.mode==='reply'){fs.writeSync(1,rule.out??'');fs.writeSync(2,rule.err??'');process.exit(rule.code??0);}if(matched&&rule.mode==='before'){fs.writeFileSync(${JSON.stringify(arrived)},'before');while(!fs.existsSync(${JSON.stringify(release)}))await new Promise(r=>setTimeout(r,10));}const result=cp.spawnSync(${JSON.stringify(real)},args,{stdio:'inherit'});if(matched&&rule.mode==='after'){fs.writeFileSync(${JSON.stringify(arrived)},'after');while(!fs.existsSync(${JSON.stringify(release)}))await new Promise(r=>setTimeout(r,10));}if(matched&&rule.mode==='overflow'){fs.writeSync(1,'x'.repeat(rule.single?140000:90000));if(!rule.single)fs.writeSync(2,'synthetic-private-diagnostic'+'y'.repeat(90000));fs.writeFileSync(${JSON.stringify(arrived)},'overflow');await new Promise(r=>setTimeout(r,60000));}process.exit(result.status??1);})().catch(()=>process.exit(2));\n`);chmodSync(join(bin,'git'),0o700);
 return {env:{...process.env,PATH:bin+':'+process.env.PATH,CREWROOM_TEST_SECRET:'synthetic-environment-sentinel'},arrived,release,stopping,calls,set(rule:Record<string,unknown>){rmSync(arrived,{force:true});rmSync(release,{force:true});rmSync(stopping,{force:true});writeFileSync(config,JSON.stringify(rule));}};
}

test('native publication compares actual remote refs and enforces expected-head leases without touching the checkout',async t=>{
 const f=fixture(t);const client=f.open();assert.equal(await client.ref('fix/crewroom/test'),undefined);
 await client.push({directory:f.checkout,head:f.head,branch:'fix/crewroom/test',expectedHead:null});assert.equal(await client.ref('fix/crewroom/test'),f.head);
 await assert.rejects(client.push({directory:f.checkout,head:f.first,branch:'fix/crewroom/test',expectedHead:null}),/not confirmed/);assert.equal(await client.ref('fix/crewroom/test'),f.head);
 await client.push({directory:f.checkout,head:f.first,branch:'fix/crewroom/test',expectedHead:f.head});assert.equal(await client.ref('fix/crewroom/test'),f.first);
 for(const branch of ['main','master','release'])await assert.rejects(client.push({directory:f.checkout,head:f.head,branch,expectedHead:null}),/protected/);
 assert.equal(git(f.checkout,'rev-parse','HEAD'),f.head);assert.equal(git(f.checkout,'branch','--show-current'),'main');assert.equal(git(f.remote,'rev-parse','main'),f.first);
 await client.close();const reopened=f.open();assert.equal(await reopened.ref('fix/crewroom/test'),f.first);
});

test('native target fetch uses a request-owned namespace and preserves operator branches, tags and FETCH_HEAD',async t=>{
 const f=fixture(t);const client=f.open();writeFileSync(join(f.checkout,'.git','FETCH_HEAD'),'operator sentinel\n');
 const destination=`refs/crewroom/${randomUUID()}/refresh/${randomUUID()}/target`;
 await client.fetchTarget({directory:f.checkout,branch:'main',destination});assert.equal(git(f.checkout,'rev-parse',destination),f.first);
 assert.equal(git(f.checkout,'rev-parse','main'),f.head);assert.equal(readFileSync(join(f.checkout,'.git','FETCH_HEAD'),'utf8'),'operator sentinel\n');assert.equal(git(f.checkout,'tag','--list'),'');
 const count=intents(f.jobs).length;
 for(const destination of ['refs/heads/main',`refs/crewroom/${randomUUID()}/target`,`refs/crewroom/${randomUUID()}/refresh/not-a-uuid/target`,`refs/crewroom/${randomUUID()}/refresh/${randomUUID()}/other`])await assert.rejects(client.fetchTarget({directory:f.checkout,branch:'main',destination}),/request-owned/);
 assert.equal(intents(f.jobs).length,count);assert.equal(git(f.checkout,'for-each-ref','--format=%(refname)','refs/crewroom/'),destination);
});

test('native publication rejects malformed refs, revisions and paths before dispatch and checks final authorization',async t=>{
 const f=fixture(t);const client=f.open();await client.ready;
 for(const name of ['','-option','a..b','a//b','a/','a.','a/.hidden','a/x.lock','a b','a\\b','é','a'.repeat(201)])await assert.rejects(client.ref(name),/branch/);
 for(const head of ['A'.repeat(40),'a'.repeat(39),'a'.repeat(65),'--help'])await assert.rejects(client.push({directory:f.checkout,head,branch:'fix/test',expectedHead:null}),/revision/);
 await assert.rejects(client.push({directory:'relative',head:f.head,branch:'fix/test',expectedHead:null}),/absolute/);
 await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/test',expectedHead:'bad'}),/revision/);
 assert.equal(intents(f.jobs).length,0);assert.equal(await client.call('guards'),0);
 await client.call('allow',{value:false});await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/test',expectedHead:null}),/revoked/);assert.equal(intents(f.jobs).length,0);assert.equal(await client.call('guards'),1);
 await client.call('allow',{value:true});await client.push({directory:f.checkout,head:f.head,branch:'fix/test',expectedHead:null});assert.equal(git(f.remote,'rev-parse','fix/test'),f.head);
});

test('native publication persists exact checked manifests, holds a unique transport lease, and makes close repeatable',async t=>{
 const f=fixture(t);const client=f.open();await client.ready;await assert.rejects(client.call('contend'),/Another publication transport/);
 await client.push({directory:f.checkout,head:f.head,branch:'fix/test',expectedHead:null});
 const [intent]=intents(f.jobs);const descriptor=intent.descriptor,spec=intent.spec;assert.equal(descriptor.root,join(f.jobs,descriptor.token));assert.equal(spec.cwd,f.checkout);
 assert.deepEqual(spec.args,['-c','core.hooksPath=/dev/null','-c','core.fsmonitor=false','-c','credential.helper=','push','--porcelain','--force-with-lease=refs/heads/fix/test:',f.remote,`${f.head}:refs/heads/fix/test`]);
 assert.equal(spec.env.GIT_SSH_COMMAND,'/usr/bin/ssh -o BatchMode=yes -o StrictHostKeyChecking=yes');assert.equal(spec.env.GIT_TERMINAL_PROMPT,'0');assert.equal(spec.env.GIT_CONFIG_GLOBAL,'/dev/null');assert.equal(spec.env.GIT_CONFIG_NOSYSTEM,'1');
 assert.ok(Object.keys(spec.env).every(k=>['PATH','HOME','LANG','SSH_AUTH_SOCK','GIT_TERMINAL_PROMPT','GIT_CONFIG_GLOBAL','GIT_CONFIG_NOSYSTEM','GIT_SSH_COMMAND'].includes(k)));
 assert.equal(createHash('sha256').update(readFileSync(join(descriptor.root,'launch.json'))).digest('hex'),descriptor.specSha256);assert.equal(statSync(join(f.jobs,descriptor.token+'.json')).mode&0o777,0o600);assert.equal(statSync(descriptor.root).mode&0o777,0o700);
 await client.call('stop');await client.call('stop');await assert.rejects(client.ref('main'),/stopping/);assert.equal(await client.call('contend'),true);assert.equal(intents(f.jobs).length,1);
});

test('native remote observations reject ambiguous output and keep diagnostics private',async t=>{
 const f=fixture(t),d=driver(f.root),client=f.open(d.env);
 for(const [code,out,expected] of [[2,'',undefined],[2,' \n',undefined],[0,f.head+'\trefs/heads/main\n',f.head]] as const){d.set({op:'ls-remote',mode:'reply',code,out});assert.equal(await client.ref('main'),expected);}
 for(const [code,out] of [[1,''],[2,'unexpected'],[0,''],[0,f.head+' refs/heads/other'],[0,f.head+' refs/heads/main\n'+f.head+' refs/heads/main'],[0,'bad refs/heads/main']]){d.set({op:'ls-remote',mode:'reply',code,out,err:'synthetic-private-diagnostic'});await assert.rejects(client.ref('main'),e=>/observation/.test((e as Error).message)&&!(e as Error).message.includes('synthetic-private'));}
 assert.ok(intents(f.jobs).every(i=>!('CREWROOM_TEST_SECRET' in i.spec.env)));
});

test('native output overflow stops the checked child and leaves an already-applied push to remote reconciliation',{timeout:30000},async t=>{
 const f=fixture(t),d=driver(f.root),client=f.open(d.env);d.set({op:'push',mode:'overflow'});const started=Date.now();
 await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/overflow',expectedHead:null}),e=>/output limit/.test((e as Error).message)&&!(e as Error).message.includes('synthetic-private'));assert.ok(Date.now()-started<12000);
 assert.equal(git(f.remote,'rev-parse','fix/overflow'),f.head);const intent=intents(f.jobs).find(i=>i.spec.args.includes('push'));assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');d.set({});assert.equal(await client.ref('fix/overflow'),f.head);
});

test('a cancelled native caller leaves its driven Git operation tracked until close drains it',{timeout:30000},async t=>{
 const f=fixture(t),d=driver(f.root),client=f.open(d.env);d.set({op:'push',mode:'before'});const id=randomUUID();const pending=client.push({directory:f.checkout,head:f.head,branch:'fix/cancelled',expectedHead:null},id);const rejected=assert.rejects(pending,/cancelled/);
 await waitFor(()=>existsSync(d.arrived));await client.cancel(id);await rejected;assert.equal((await inspectCheckedOwned(intents(f.jobs)[0].descriptor)).state,'live');await client.call('stop');assert.equal((await inspectCheckedOwned(intents(f.jobs)[0].descriptor)).state,'stopped');assert.equal(git(f.remote,'for-each-ref','--format=%(objectname)','refs/heads/fix/cancelled'),'');await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/late',expectedHead:null}),/stopping/);
});

test('actual controller SIGKILL before and after push effect recovers stopped ownership without replaying the old write',{timeout:60000},async t=>{
 const f=fixture(t),d=driver(f.root);
 for(const mode of ['before','after']){
  const client=f.open(d.env);d.set({op:'push',mode});const branch='fix/killed-'+mode;const pending=client.push({directory:f.checkout,head:f.head,branch,expectedHead:null});const rejected=assert.rejects(pending,/exited/);await waitFor(()=>existsSync(d.arrived));
  const intent=intents(f.jobs).find(i=>i.spec.args.at(-1)===`${f.head}:refs/heads/${branch}`);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'live');await client.kill();await rejected;assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'live');if(mode==='after')renameSync(join(f.root,'driver','git'),join(f.root,'retired-git'));d.set({});
  const reopened=f.open(d.env);assert.equal(await reopened.ref(branch),mode==='after'?f.head:undefined);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');assert.equal(readFileSync(d.calls,'utf8').trim().split('\n').map(s=>JSON.parse(s)).filter(a=>a[6]==='push'&&a.at(-1)===`${f.head}:refs/heads/${branch}`).length,1);await reopened.close();
 }
});

test('native recovery refuses changed command identity, unfamiliar legacy roots and a replaced transport lock',async t=>{
 const f=fixture(t);let client=f.open();await client.ref('main');await client.close();
 const [intent]=intents(f.jobs),file=join(f.jobs,intent.descriptor.token+'.json'),original=readFileSync(file);const changed=structuredClone(intent);changed.spec.args.push('--changed');writeFileSync(file,JSON.stringify(changed));client=f.open();f.expectedCloseErrors.add(client);await assert.rejects(client.ref('main'),/identity changed/);await client.close().catch(()=>{});writeFileSync(file,original);
 const legacy=join(f.jobs,randomUUID());mkdirSync(legacy);writeFileSync(join(legacy,'stopped.json'),'{}');client=f.open();f.expectedCloseErrors.add(client);await assert.rejects(client.ref('main'),/legacy/);await client.close().catch(()=>{});rmSync(legacy,{recursive:true});
 client=f.open();await client.ready;const lock=join(f.jobs,'transport.lock'),saved=join(f.root,'saved-lock');renameSync(lock,saved);writeFileSync(lock,'');await assert.rejects(client.ref('main'),/ownership changed/);rmSync(lock);renameSync(saved,lock);await client.close();
});

function helperWrapper(root:string,mode:'quiet'|'ignore-term'){
 const file=join(root,'helper-'+mode),binary=realpathSync('target/debug/crewroom-process');writeFileSync(file,`#!/bin/sh\n${mode==='ignore-term'?"trap '' TERM\n":''}exec '${binary.replaceAll("'","'\\''")}' "$@"${mode==='quiet'?' >/dev/null':''}\n`);chmodSync(file,0o700);return file;
}

test('late authorization and close during old-child recovery prevent a new publication dispatch',{timeout:60000},async t=>{
 const f=fixture(t),d=driver(f.root),helper=helperWrapper(f.root,'ignore-term');
 for(const action of ['deny','close']){
  const old=f.open(d.env,helper);d.set({op:'push',mode:'before',ignoreTerm:true});const branch='fix/recovery-'+action;const pending=old.push({directory:f.checkout,head:f.head,branch,expectedHead:null});const died=assert.rejects(pending,/exited/);await waitFor(()=>existsSync(d.arrived));const before=intents(f.jobs).length;await old.kill();await died;
  const current=f.open(d.env);await current.ready;const late=current.push({directory:f.checkout,head:f.head,branch:'fix/late-'+action,expectedHead:null});const rejected=assert.rejects(late,/revoked|stopping/);await waitFor(()=>existsSync(d.stopping));
  if(action==='deny')await current.call('allow',{value:false});else await current.call('stop');await rejected;assert.equal(intents(f.jobs).length,before);assert.equal(readdirSync(f.jobs).filter(n=>!n.endsWith('.json')&&n!=='transport.lock').length,before,'No directory for an unpersisted cancelled command');assert.equal(git(f.remote,'for-each-ref','--format=%(refname)','refs/heads/fix/late-'),'');await current.close();
 }
});

test('a missing live helper report never confirms an already-applied publication',{timeout:30000},async t=>{
 const f=fixture(t),client=f.open(undefined,helperWrapper(f.root,'quiet'));
 await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/no-report',expectedHead:null}),/outcome was not confirmed/);assert.equal(git(f.remote,'rev-parse','fix/no-report'),f.head);
 const [intent]=intents(f.jobs);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');assert.equal(JSON.parse(readFileSync(join(intent.descriptor.root,'capture.json'),'utf8')).stdout.truncated,false);await client.close();const reopened=f.open();assert.equal(await reopened.ref('fix/no-report'),f.head);
});

test('native publication deadline stops a stalled command and retains its durable intent',{timeout:45000},async t=>{
 const f=fixture(t),d=driver(f.root),client=f.open(d.env);d.set({op:'push',mode:'before'});const started=Date.now();await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/deadline',expectedHead:null}),/timed out/);assert.ok(Date.now()-started>=29000);assert.ok(Date.now()-started<40000);const [intent]=intents(f.jobs);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');assert.equal(git(f.remote,'for-each-ref','--format=%(refname)','refs/heads/fix/deadline'),'');
});

test('recovery seals a descriptor whose process directory was never prepared and never replays it',async t=>{
 const f=fixture(t);let client=f.open();await client.push({directory:f.checkout,head:f.head,branch:'fix/unprepared',expectedHead:null});await client.close();const [intent]=intents(f.jobs);rmSync(intent.descriptor.root,{recursive:true});
 client=f.open();assert.equal(await client.ref('fix/unprepared'),f.head);assert.deepEqual(JSON.parse(readFileSync(join(intent.descriptor.root,'seal.json'),'utf8')),{token:intent.descriptor.token,specSha256:intent.descriptor.specSha256});assert.equal(existsSync(join(intent.descriptor.root,'launch.json')),false);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');assert.equal(intents(f.jobs).filter(i=>i.spec.args.includes('push')).length,1);
});

test('single-stream overflow stops promptly while the exact retained output limit remains accepted',{timeout:30000},async t=>{
 const f=fixture(t),d=driver(f.root),client=f.open(d.env);d.set({op:'push',mode:'overflow',single:true});const started=Date.now();await assert.rejects(client.push({directory:f.checkout,head:f.head,branch:'fix/single-overflow',expectedHead:null}),/output limit/);assert.ok(Date.now()-started<12000);assert.equal(git(f.remote,'rev-parse','fix/single-overflow'),f.head);const [intent]=intents(f.jobs);assert.equal(existsSync(join(intent.descriptor.root,'stdout.truncated')),true);assert.equal((await inspectCheckedOwned(intent.descriptor)).state,'stopped');
 // A synthetic Git reply exercises the byte boundary only; no remote effect is claimed.
 d.set({op:'push',mode:'reply',out:'x'.repeat(128*1024),code:0});await client.push({directory:f.checkout,head:f.head,branch:'fix/exact-limit',expectedHead:null});assert.equal(git(f.remote,'for-each-ref','--format=%(refname)','refs/heads/fix/exact-limit'),'');
});
