/** Shared disposable Git/SQLite fixture. Agent identities are records, not runner proof. */
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {existsSync,mkdtempSync,readFileSync,readdirSync,realpathSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {type TestContext} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {DatabaseSync} from 'node:sqlite';
import {seedMessages} from './messaging-fixture';
import {NativeRepository} from './repository-adapter';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
export function fixture(t:TestContext) {
 const root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-target-')));
 const seed=seedMessages(join(root,'operator'));
 const gitAt=(path:string,...args:string[])=>execFileSync('git',['-C',path,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid',...args],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trimEnd();
 const git=(...args:string[])=>gitAt(seed.root,...args);
 writeFileSync(join(seed.root,'source.txt'),'before\n');writeFileSync(join(seed.root,'AGENTS.md'),'Accepted instructions\n');git('add','.');git('commit','-m','source');
 const config={database:seed.database,root:join(root,'owned'),authority:join(root,'data')};let c=new NativeRepository(config);
 const beforeCleanup:Array<()=>Promise<void>>=[];
 const intents=(run=seed.run.id)=> {const dir=join(config.root,'.git-operations',run);return existsSync(dir)?readdirSync(dir).filter(n=>n.endsWith('.json')).map(n=>JSON.parse(readFileSync(join(dir,n),'utf8'))):[];};const descriptors=()=>intents().map(r=>r.descriptor);
 t.after(async()=>{await c.close();for(const cleanup of beforeCleanup)await cleanup();for(const d of (existsSync(join(config.root,'.git-operations'))?readdirSync(join(config.root,'.git-operations')).flatMap(run=>intents(run).map(i=>i.descriptor)):[])){const state=await inspectCheckedOwned(d);if(state.state!=='stopped')await stopOwned(d.root);assert.equal(state.state,'stopped','Fixture left a live or uncertain Git process');}rmSync(root,{recursive:true,force:true});});
 const f={root,seed,config,git,gitAt,descriptors,intents,beforeCleanup,get c(){return c;},async reopen(options:Record<string,unknown>={}){await c.close();c=new NativeRepository({...config,...options});await c.ready;},
  async candidate(file='source.txt',content='implementation\n',beforeCompletion?:(work:any)=>void) {
   const work=await c.command('prepare',{target:seed.run.id,input:{mode:'new'}});await c.command('activate',{target:seed.run.id});await c.command('claim',{target:seed.recipient.attempt.id});
   const read=await c.command('read',{target:seed.run.id,path:file});await c.command('edit',{target:seed.recipient.attempt.id,key:'edit',input:{path:file,expectedHash:read.sha256,content}});
   beforeCompletion?.(work);
   const completion=await c.command('request_completion',{target:seed.recipient.attempt.id,key:'finish',input:{summary:'Candidate source'}});
   await c.command('pause',{target:seed.run.id});for(const a of [seed.sender,seed.recipient])await c.command('ownership',{target:a.attempt.id});await c.command('finish_pause',{target:seed.run.id});await c.command('release',{target:seed.recipient.attempt.id});
   const candidate=await c.command('complete',{target:seed.run.id,key:completion.id});return {work,candidate};
  },
  move(file='target.txt',content='target\n'){writeFileSync(join(seed.root,file),content);git('add',file);git('commit','-m','target move');return git('rev-parse','HEAD');},
  refresh(source:any,key=randomUUID(),targetRef='refs/heads/main',supersedes?:any){return c.command('refresh',{target:seed.run.id,key,targetRef,sourceCandidateId:source.id,...(supersedes?{supersedes}:{})});},
  record(){return c.command('work_record',{target:seed.run.id});},
 };
 return f;
}
export function alter(f:ReturnType<typeof fixture>,table:string,id:string,change:(data:any)=>void){const db=new DatabaseSync(f.seed.database);try{const data=JSON.parse((db.prepare(`SELECT data FROM ${table} WHERE ${table==='repository_work'?'run_id':'id'}=?`).get(id)as any).data);change(data);db.prepare(`UPDATE ${table} SET data=? WHERE ${table==='repository_work'?'run_id':'id'}=?`).run(JSON.stringify(data),id);}finally{db.close();}}
export async function waitFor(path:string,timeoutMs=10000){const until=Date.now()+timeoutMs;while(!existsSync(path)){assert.ok(Date.now()<until,`Timed out waiting for ${path}`);await delay(5);}}

