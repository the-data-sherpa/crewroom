import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import { once } from 'node:events';
import { existsSync,mkdirSync,mkdtempSync,readFileSync,readdirSync,realpathSync,rmSync,writeFileSync } from 'node:fs';
import { createServer,request } from 'node:http';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import test from 'node:test';
import { NativeHost,backend,until } from './host-fixture';
import { ContainerCommand } from './container-adapter';
import { pythonCheckImage,requireDockerEngine } from './docker-engine';
const engine=await requireDockerEngine();
const image=pythonCheckImage();
const detached="import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env={}); print('real-host-ready',flush=True);time.sleep(60)";

test('native host cancels an actual Docker command with a detached child and retains its final receipt',{timeout:45000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const workspace=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-host-workspace-')));
  const h=new NativeHost(b.origin,{}, {commandEngine:engine,tools:[{id:'detached',image,workspace,command:'/usr/local/bin/python3',args:['-c',detached]}]});
  t.after(async()=>{await h.close();rmSync(workspace,{recursive:true,force:true});});await h.ready();
  await h.input('command',{kind:'tool',name:'run_command',arguments:{commandId:'detached'},cancelGate:'cancel-running'});
  const root=await until(()=>existsSync(join(h.root,'tools'))&&readdirSync(join(h.root,'tools'))[0]&&join(h.root,'tools',readdirSync(join(h.root,'tools'))[0]));
  const job=new ContainerCommand(root,engine);await until(async()=> (await job.inspect()).state==='running');
  await until(()=>existsSync(join(root,'live.log'))&&readFileSync(join(root,'live.log'),'utf8').includes('real-host-ready'));
  writeFileSync(join(h.root,'cancel-running-release'),'cancel');assert.equal((await h.terminal('command')).status,'error');
  const receipt=await job.receipt();assert.equal(receipt.started,true);assert.equal(receipt.completed,false);assert.equal(receipt.timedOut,false);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).stopRequested,true);assert.match(receipt.output,/real-host-ready/);assert.equal((await job.inspect()).state,'stopped');
  assert.equal(readFileSync(join(root,'output.log'),'utf8'),receipt.output);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).removed,true);
});

test('unavailable engine keeps host close unresolved until actual container ownership is reconciled',{timeout:60000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const support=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-host-engine-')));const workspace=join(support,'workspace');mkdirSync(workspace);const socket=join(support,'engine.sock');let available=true;
  const proxy=createServer((req,res)=>{
    if(!available){res.destroy();return;}
    const upstream=request({socketPath:engine.socket,path:req.url,method:req.method,headers:req.headers},response=>{res.writeHead(response.statusCode!,response.headers);response.pipe(res);});
    upstream.on('error',()=>res.destroy());req.pipe(upstream);res.on('close',()=>upstream.destroy());
  });proxy.listen(socket);await once(proxy,'listening');
  const h=new NativeHost(b.origin,{}, {commandEngine:{socket},tools:[{id:'detached',image,workspace,command:'/usr/local/bin/python3',args:['-c',detached]}]});
  t.after(async()=>{available=true;await h.close();proxy.closeAllConnections();proxy.close();rmSync(support,{recursive:true,force:true});});await h.ready();
  await h.input('command',{kind:'tool',name:'run_command',arguments:{commandId:'detached'}});
  const root=await until(()=>existsSync(join(h.root,'tools'))&&readdirSync(join(h.root,'tools'))[0]&&join(h.root,'tools',readdirSync(join(h.root,'tools'))[0]));
  const job=new ContainerCommand(root,engine);await until(async()=> (await job.inspect()).state==='running');available=false;
  const first=await h.request({op:'close'});assert.equal(first.status,409);assert.notEqual(first.value.stopped,true);assert.equal(h.proc.exitCode,null);assert.equal((await job.inspect()).state,'running');
  available=true;await h.value({op:'close'});await h.exited;assert.equal((await job.inspect()).state,'stopped');assert.equal((await job.receipt()).completed,false);assert.equal((await job.receipt()).timedOut,false);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).stopRequested,true);
});

test('external cleanup after abrupt host death stops retained Docker descendants and saves the receipt',{timeout:45000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const workspace=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-host-death-')));
  const h=new NativeHost(b.origin,{}, {commandEngine:engine,tools:[{id:'detached',image,workspace,command:'/usr/local/bin/python3',args:['-c',detached]}]});
  t.after(async()=>{await h.close();rmSync(workspace,{recursive:true,force:true});});await h.ready();await h.input('survivor',{kind:'tool',name:'run_command',arguments:{commandId:'detached'}});
  const root=await until(()=>existsSync(join(h.root,'tools'))&&readdirSync(join(h.root,'tools'))[0]&&join(h.root,'tools',readdirSync(join(h.root,'tools'))[0]));const job=new ContainerCommand(root,engine);
  await until(()=>existsSync(join(root,'live.log'))&&readFileSync(join(root,'live.log'),'utf8').includes('real-host-ready'));
  h.proc.kill('SIGKILL');await h.exited;assert.equal((await job.inspect()).state,'running');assert.equal((await job.stop()).state,'stopped');
  const receipt=await job.receipt();assert.equal(receipt.started,true);assert.equal(receipt.completed,false);assert.match(receipt.output,/real-host-ready/);await job.remove();assert.equal((await job.inspect()).state,'stopped');
});


test('configured command, prepared check and read-only shell complete with final receipts before result delivery',{timeout:45000},async t=>{
  const workspace=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-host-finished-')));let h:NativeHost;const routes:string[]=[];
  const spec={image,workspace,command:'/usr/local/bin/python3',args:['-c',"print('actual-host-command-output')"]};
  const server=createServer(async(req,res)=>{
    let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);routes.push(req.url!);
    if(req.url==='/internal/coding/check-finish') {
      const root=join(h.root,'tools',createHash('sha256').update(body.toolCallId).digest('hex'));const receipt=await new ContainerCommand(root,engine).receipt();
      assert.equal(receipt.completed,true);assert.equal(receipt.exitCode,0);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).removed,true);
      res.end('{"fixtureCompletion":true}');
    } else res.end(JSON.stringify({spec:{...spec,...(req.url==='/internal/coding/shell'?{readOnlyWorkspace:true}:{})}}));
  });server.listen(0,'127.0.0.1');await once(server,'listening');
  h=new NativeHost(`http://127.0.0.1:${(server.address() as any).port}`,{}, {coding:true,commandEngine:engine,tools:[{id:'configured',...spec}]});
  t.after(async()=>{await h.close();server.close();rmSync(workspace,{recursive:true,force:true});});await h.ready();
  for(const [name,args] of [['run_command',{commandId:'configured'}],['run_check',{checkId:'configured'}],['run_shell',{command:'python fixture'}]] as const) {
    await h.input(name,{kind:'tool',id:name,toolCallId:name,name,arguments:args});assert.equal((await h.terminal(name)).status,'completed');
    const result=h.requests().find(v=>v.type==='host_tool_result'&&v.id===name);assert.ok(!result.isError);const value=JSON.parse(result.result.content[0].text);assert.match(name==='run_check'?value.outputPreview:value.output,/actual-host-command-output/);
    if(name==='run_check')assert.equal(value.fixtureCompletion,true);
    if(name==='run_shell')assert.match(value.note,/not check evidence/);
  }
  assert.deepEqual(routes,['/internal/coding/check-prepare','/internal/coding/check-finish','/internal/coding/shell']);
});
