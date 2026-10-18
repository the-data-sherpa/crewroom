import assert from 'node:assert/strict';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { appendFileSync, chmodSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import test from 'node:test';
import { savedInputs } from '../../server/runtime/inputs';
class Journal {
  proc: ChildProcessWithoutNullStreams;
  pending = new Map<number,(v:any)=>void>(); next=0;
  ready: Promise<void>; stderr=''; stopped: Promise<any>;
  constructor(root:string) {
    this.proc=spawn('target/debug/examples/host-journal-fixture',[],{stdio:'pipe'});
    this.stopped=once(this.proc,'exit');
    this.proc.stderr.on('data',chunk=>{this.stderr+=chunk;});
    this.ready=new Promise((resolve,reject)=>{
      this.proc.once('exit',()=>reject(new Error(this.stderr||'Journal exited before ready')));
      const lines=createInterface({input:this.proc.stdout});
      lines.on('line',line=>{const v=JSON.parse(line);if(v.ready)resolve();else{this.pending.get(v.id)?.(v);this.pending.delete(v.id);}});
    });
    this.proc.stdin.write(JSON.stringify({root})+'\n');
  }
  async command(op:string, args:Record<string,unknown>={}) {
    await this.ready; const id=++this.next;
    const reply=new Promise<any>(resolve=>this.pending.set(id,resolve));
    this.proc.stdin.write(JSON.stringify({id,op,...args})+'\n');
    return reply;
  }
  async value(op:string,args:Record<string,unknown>={}) { const r=await this.command(op,args);assert.equal(r.ok,true,r.error);return r.value; }
  async close(kill=false) {if(this.proc.exitCode===null&&this.proc.signalCode===null){if(kill)this.proc.kill('SIGKILL');else this.proc.stdin.end();}await this.stopped;}
}
const make=()=>realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-host-journal-')));

test('native durable admission survives process death and reads through the retained TypeScript parser', {timeout:20000},async t=>{
  const root=make();t.after(()=>rmSync(root,{recursive:true,force:true}));
  let journal=new Journal(root);t.after(()=>journal.close());
  assert.equal(await journal.value('accept',{key:'unknown',message:'body 😀'}),true);
  await journal.close(true);assert.equal(journal.proc.signalCode,'SIGKILL');
  assert.deepEqual(savedInputs(root),[{id:'unknown',message:'body 😀',disposition:'accepted'}]);
  journal=new Journal(root);
  assert.equal((await journal.command('accept',{key:'unknown',message:'body 😀'})).ok,false);
  assert.equal((await journal.command('accept',{key:'unknown',message:'changed'})).ok,false);
  await journal.value('completed',{key:'unknown',status:'aborted'});await journal.value('received',{key:'unknown'});
  assert.equal(await journal.value('accept',{key:'unknown',message:'body 😀'}),false);
  await journal.close();assert.equal(savedInputs(root)[0].disposition,'aborted');
  appendFileSync(join(root,'inputs.jsonl'),JSON.stringify({id:'unknown',disposition:'received'})+'\n',{flush:true});
  journal=new Journal(root);assert.equal(await journal.value('status',{key:'unknown'}),'aborted');
  assert.equal(await journal.value('accept',{key:'next',message:'next input'}),true);await journal.value('received',{key:'next'});await journal.close();
  assert.deepEqual(savedInputs(root).map(v=>v.disposition),['aborted','received']);
});

test('independent observers retain complete large events across host-journal process death', {timeout:20000},async t=>{
  const root=make();t.after(()=>rmSync(root,{recursive:true,force:true}));
  writeFileSync(join(root,'events.jsonl'),JSON.stringify({sequence:1,event:{type:'prior_receipt'}})+'\n',{mode:0o600});chmodSync(join(root,'events.jsonl'),0o600);
  let journal=new Journal(root);t.after(()=>journal.close());
  const event={type:'host_tool_call',toolCallId:'large',arguments:{content:'中'.repeat(1_500_000)},token:'fixture-secret',note:'fixture-secret'};
  assert.equal(await journal.value('event',{event}),2);await journal.close(true);
  journal=new Journal(root);
  const page=await journal.value('page',{after:1});assert.equal(page.length,1);assert.equal(page[0].sequence,2);
  assert.equal(page[0].event.arguments.content,event.arguments.content);assert.equal(page[0].event.token,undefined);assert.equal(page[0].event.note,'[redacted]');
  assert.deepEqual(await journal.value('page',{after:1}),page);assert.equal(await journal.value('event',{event:{type:'after_reopen'}}),3);
  await journal.close();const lines=readFileSync(join(root,'events.jsonl'),'utf8').trimEnd().split('\n').map(line=>JSON.parse(line));
  assert.deepEqual(lines.map(v=>v.sequence),[1,2,3]);assert.deepEqual(lines[1],page[0]);
});

test('actual partial write poisons admission and reopen refuses the damaged journal', {timeout:20000},async t=>{
  for(const type of ['input','event']){
    const root=make();t.after(()=>rmSync(root,{recursive:true,force:true}));
    const journal=new Journal(root);t.after(()=>journal.close());
    await journal.value('file_limit',{bytes:10});
    const result=type==='input'?await journal.command('accept',{key:'key',message:'not acknowledged'}):await journal.command('event',{event:{type:'not acknowledged'}});
    assert.equal(result.ok,false);assert.match(result.error,/File too large|file too large/);
    if(type==='input')assert.equal((await journal.command('status',{key:'key'})).ok,false,'A failed append must not report absent dispatch');
    else assert.equal(await journal.value('sequence'),0);
    await journal.value('file_limit',{});
    const repeat=type==='input'?await journal.command('accept',{key:'key',message:'not acknowledged'}):await journal.command('event',{event:{type:'retry'}});
    assert.equal(repeat.ok,false);assert.match(repeat.error,/uncertain/);await journal.close(true);
    const broken=spawnSync('target/debug/examples/host-journal-fixture',[],{input:JSON.stringify({root})+'\n',encoding:'utf8',timeout:5000});
    assert.equal(broken.status,1);assert.match(broken.stderr,/incomplete/);
    if(type==='input')assert.throws(()=>savedInputs(root));
  }
});
