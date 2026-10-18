import assert from 'node:assert/strict';
import {createServer,request} from 'node:http';
import {once} from 'node:events';
import {existsSync,mkdirSync,readFileSync,readdirSync,symlinkSync,unlinkSync} from 'node:fs';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import test from 'node:test';
import {supervisorFixture,until,image,socket} from './supervisor-support';
import {ContainerCommand} from './container-adapter';
const detached="import subprocess,sys,time;subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env={});print('supervised-container-ready',flush=True);time.sleep(60)";

test('restarted supervisor stops a real Docker descendant after controller and host SIGKILL and preserves its receipt',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);const workspace=join(f.root,'workspace');mkdirSync(workspace);
 const identity=await f.launch(0,{}, {commandEngine:{socket},tools:[{id:'detached',image:image(),workspace,command:'/usr/local/bin/python3',args:['-c',detached]}]});
 await f.input('container',{kind:'tool',name:'run_command',arguments:{commandId:'detached'}});
 const root=await until(()=>existsSync(join(f.attemptRoot(),'tools'))&&readdirSync(join(f.attemptRoot(),'tools'))[0]&&join(f.attemptRoot(),'tools',readdirSync(join(f.attemptRoot(),'tools'))[0]));const job=new ContainerCommand(root,{socket});
 await until(()=>existsSync(join(root,'live.log'))&&readFileSync(join(root,'live.log'),'utf8').includes('supervised-container-ready'));
 await f.kill();process.kill(identity.hostPid,'SIGKILL');assert.equal((await job.inspect()).state,'running');await f.reopen();assert.equal((await f.stop()).stopped,true);
 const receipt=await job.receipt();assert.equal(receipt.started,true);assert.equal(receipt.completed,false);assert.equal(receipt.timedOut,false);assert.match(receipt.output,/supervised-container-ready/);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).removed,true);
});

test('unavailable Docker engine retains uncertain ownership until a repeated native Stop reconciles it',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);await f.c.close();const proxySocket=join(f.root,'engine.sock');let available=true;
 const proxy=createServer((req,res)=>{if(!available){res.destroy();return;}const upstream=request({socketPath:socket,path:req.url,method:req.method,headers:req.headers},response=>{res.writeHead(response.statusCode!,response.headers);response.pipe(res);});upstream.on('error',()=>res.destroy());req.pipe(upstream);res.on('close',()=>upstream.destroy());});
 proxy.listen(proxySocket);await once(proxy,'listening');t.after(()=>{proxy.closeAllConnections();proxy.close();});await f.reopen({socket:proxySocket});
 const workspace=join(f.root,'workspace');mkdirSync(workspace);const identity=await f.launch(0,{}, {commandEngine:{socket:proxySocket},tools:[{id:'detached',image:image(),workspace,command:'/usr/local/bin/python3',args:['-c',detached]}]});await f.input('container',{kind:'tool',name:'run_command',arguments:{commandId:'detached'}});
 const root=await until(()=>existsSync(join(f.attemptRoot(),'tools'))&&readdirSync(join(f.attemptRoot(),'tools'))[0]&&join(f.attemptRoot(),'tools',readdirSync(join(f.attemptRoot(),'tools'))[0]));const job=new ContainerCommand(root,{socket});await until(async()=>(await job.inspect()).state==='running');
 await f.kill();process.kill(identity.hostPid,'SIGKILL');available=false;await f.reopen({socket:proxySocket});assert.equal((await f.stop()).stopped,false);assert.equal((await f.c.command('attempt',{target:f.seeds[0].attempt.id})).ownership,'uncertain');assert.equal((await job.inspect()).state,'running');
 available=true;assert.equal((await f.stop()).stopped,true);assert.equal((await job.inspect()).state,'stopped');assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).removed,true);
});

test('one failed SQLite ownership receipt cannot skip another live host during shutdown',{timeout:25000},async t=>{
 const f=await supervisorFixture(t,2,true);await f.launch(0);await f.launch(1);const db=new DatabaseSync(f.database);
 db.exec(`CREATE TRIGGER fail_stop BEFORE UPDATE ON attempts WHEN NEW.id='${f.seeds[0].attempt.id}' AND json_extract(NEW.data,'$.ownership')='stopped' BEGIN SELECT RAISE(ABORT,'injected stop receipt failure'); END;`);
 try {
  await assert.rejects(f.stop(),/injected stop receipt failure/);
  assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'stopped');assert.equal((await f.c.command('owned',{target:f.seeds[1].attempt.id})).state,'stopped');assert.equal((await f.c.command('attempt',{target:f.seeds[1].attempt.id})).ownership,'stopped');
 }finally{db.exec('DROP TRIGGER fail_stop');db.close();}
 assert.equal((await f.stop()).stopped,true);
});

test('mismatched saved PIDs are never signalled and retain an ownership hold after owned children stop',{timeout:20000},async t=>{
 const f=await supervisorFixture(t);const identity=await f.launch();const db=new DatabaseSync(f.database);const id=f.seeds[0].attempt.id;
 const change=(pid:number)=>{const row=db.prepare('SELECT data FROM attempts WHERE id=?').get(id)!;const attempt={...JSON.parse(String(row.data)),hostPid:pid};db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt),id);};
 try {change(f.c.proc.pid!);assert.equal((await f.stop()).stopped,false);assert.equal((await f.c.command('attempt',{target:id})).ownership,'uncertain');change(identity.hostPid);assert.equal((await f.stop()).stopped,true);}finally{db.close();}
});

test('a replaced or dangling tool directory retains an ownership hold after exact process cleanup',{timeout:30000},async t=>{
 for(const dangling of [false,true]) {
  const f=await supervisorFixture(t);await f.launch();const target=join(f.root,'foreign-tools');if(!dangling)mkdirSync(target);
  const path=join(f.attemptRoot(),'tools');symlinkSync(target,path,'dir');
  try {
   const result=await f.stop();
   assert.equal(result.stopped,false,'A changed command directory cannot prove absent container ownership');
   assert.equal((await f.c.command('attempt',{target:f.seeds[0].attempt.id})).ownership,'uncertain');
   assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'stopped','Exact owned processes still stop');
  }finally{unlinkSync(path);assert.equal((await f.stop()).stopped,true);}
 }
});
