import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {existsSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import test from 'node:test';
import {supervisorFixture,until} from './supervisor-support';

test('bounded host control rejects foreign routes, redirects, oversized bodies and stalled responses',{timeout:20000},async t=>{
 const f=await supervisorFixture(t,0);let mode='normal',requests=0;
 const server=createServer(async(req,res)=>{requests++;for await(const _ of req){}if(mode==='stall')return;if(mode==='redirect'){res.writeHead(307,{Location:f.b.origin});res.end();return;}if(mode==='large'){res.writeHead(200,{'Content-Length':String(128*1024*1024)});res.end();return;}if(mode==='malformed'){res.end('{bad');return;}assert.equal(req.headers.authorization,'Bearer synthetic-control-token');res.end('{"ok":true}');});
 server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>{server.closeAllConnections();server.close();});const url=`http://127.0.0.1:${(server.address() as any).port}`,token='synthetic-control-token';
 for(const bad of [url+'/route',url+'?leak=1',url+'#fragment',url.replace('127.0.0.1','localhost'),url.replace('http:','https:'),url.replace('http://','http://user:pass@')])await assert.rejects(f.c.command('client',{url:bad,token,input:{op:'identity'}}),/origin|loopback/);
 assert.equal(requests,0);assert.deepEqual(await f.c.command('client',{url,token,input:{op:'identity'}}),{ok:true});
 for(const value of ['redirect','large','malformed','stall']){mode=value;await assert.rejects(f.c.command('client',{url,token,input:{op:'identity'}}),/refused|bounded|JSON|timed out/);}
 assert.equal(f.b.requests.length,0,'No credential may follow a redirect');
});

test('slow observations with exact live processes stay degraded and recover without changing attempt identity',{timeout:35000},async t=>{
 const f=await supervisorFixture(t);const identity=await f.launch();await f.c.command('activate',{target:f.seeds[0].run.id});await f.kill();
 const config=JSON.parse(readFileSync(join(f.attemptRoot(),'host.json'),'utf8'));let held=true,observations=0;
 const proxy=createServer(async(req,res)=>{let body='';for await(const chunk of req)body+=chunk;const input=JSON.parse(body);if(input.op==='events'&&held){observations++;return;}const response=await fetch(identity.hostUrl,{method:'POST',headers:{Authorization:`Bearer ${config.token}`,'Content-Type':'application/json',Origin:identity.hostUrl},body});res.writeHead(response.status);res.end(await response.text());});
 proxy.listen(0,'127.0.0.1');await once(proxy,'listening');t.after(()=>{proxy.closeAllConnections();proxy.close();});const port=(proxy.address() as any).port;
 writeFileSync(join(f.attemptRoot(),'endpoint.json'),JSON.stringify({port}),{mode:0o600});const db=new DatabaseSync(f.database);try{const row=db.prepare('SELECT data FROM attempts WHERE id=?').get(f.seeds[0].attempt.id)!;const attempt={...JSON.parse(String(row.data)),hostUrl:`http://127.0.0.1:${port}`};db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt),attempt.id);}finally{db.close();}
 await f.reopen();assert.equal((await f.c.command('reattach',{target:f.seeds[0].attempt.id})).reattached,true);await f.observed(0,'degraded');assert.ok(observations>0);
 const retained=await f.c.command('attempt',{target:f.seeds[0].attempt.id});for(const key of ['hostPid','ompPid','sessionId'])assert.equal(retained[key],identity[key]);assert.equal(retained.ownership,'held');assert.equal((await f.c.command('owned',{target:retained.id})).state,'live');
 held=false;await f.input('wake',{});await f.observed();assert.equal((await f.c.command('runtime',{target:f.seeds[0].run.id})).attempts.length,1);
});

test('a mismatched live session is an identity hold even when both saved processes are still live',{timeout:20000},async t=>{
 const f=await supervisorFixture(t);await f.launch();await f.kill();const db=new DatabaseSync(f.database);
 try{const row=db.prepare('SELECT data FROM attempts WHERE id=?').get(f.seeds[0].attempt.id)!;const attempt={...JSON.parse(String(row.data)),sessionId:'wrong-session'};db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt),attempt.id);}finally{db.close();}
 await f.reopen();await assert.rejects(f.c.command('reattach',{target:f.seeds[0].attempt.id}),/identity differs/);assert.equal((await f.stop()).stopped,true);
});

test('controller death before delayed tmux dispatch seals the old launch and rejects its late helper',{timeout:25000},async t=>{
 const f=await supervisorFixture(t);await f.c.close();const wrapper=join(f.root,'tmux-wrapper');
 writeFileSync(wrapper,`#!/usr/bin/env python3\nimport os,sys,time\nfrom pathlib import Path\nroot=Path(${JSON.stringify(f.root)})\nif 'new-session' in sys.argv or 'new-window' in sys.argv:\n (root/'tmux-waiting').write_text(str(os.getpid()))\n deadline=time.monotonic()+15\n while not (root/'tmux-release').exists() and time.monotonic()<deadline:time.sleep(.01)\nos.execv('/opt/homebrew/bin/tmux',['tmux',*sys.argv[1:]])\n`,{mode:0o700});
 await f.reopen({tmux:wrapper});const launching=f.launch();const failed=assert.rejects(launching,/exited/);await until(()=>existsSync(join(f.root,'tmux-waiting')));await f.kill();await failed;
 await f.reopen();assert.equal((await f.stop()).stopped,true);writeFileSync(join(f.root,'tmux-release'),'go');
 const wrapperPid=Number(readFileSync(join(f.root,'tmux-waiting'),'utf8'));await until(()=>{try{process.kill(wrapperPid,0);return false;}catch{return true;}});
 assert.equal(existsSync(join(f.attemptRoot(),'host-process/owner.json')),false);assert.equal(existsSync(join(f.attemptRoot(),'omp-process/launch.json')),false);assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'stopped');
});
