import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import type {TestContext} from 'node:test';
import {randomUUID} from 'node:crypto';
import {fixture} from './target-fixture';

export async function ready(t:TestContext,pinned=false) {
 const f=fixture(t);const {candidate,work}=await f.candidate();const target=f.move('source.txt','Target conflict\n');const observer=join(f.root,'observer');mkdirSync(observer);writeFileSync(join(observer,'result.json'),JSON.stringify({head:target}));const pin=`refs/crewroom/${f.seed.run.id}/${randomUUID()}`;if(pinned){f.git('update-ref',pin,target);f.git('reset','--hard',candidate.revision.base);await f.reopen({targetObserver:observer});}const r=(await f.refresh(candidate,randomUUID(),pinned?pin:'refs/heads/main')).refresh;
 const runtime=await f.c.command('runtime',{target:f.seed.run.id});for(const a of runtime.assignments)await f.c.command('replace',{target:a.id,input:{automatic:false}});
 await f.c.command('continue',{target:f.seed.run.id});await f.c.command('activate',{target:f.seed.run.id});
 const next=await f.c.command('runtime',{target:f.seed.run.id});const coordinator=next.assignments.find((a:any)=>a.coordinator).currentAttemptId;const writer=next.assignments.find((a:any)=>!a.coordinator).currentAttemptId;
 await f.c.command('claim_implementation',{target:coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});await f.c.command('edit',{target:writer,key:'resolution',input:{path:'source.txt',expectedHash:read.sha256,content:'Resolved source\n'}});
 await f.c.command('continue_conflict',{target:writer,key:'advance',input:{requestId:r.requestId}});
 const completion=await f.c.command('request_completion',{target:writer,key:'resolved',input:{summary:'Resolved target conflict'}});
 assert.deepEqual(await f.c.command('request_completion',{target:writer,key:'resolved',input:{summary:'Resolved target conflict'}}),completion);
 await f.c.command('pause',{target:f.seed.run.id});for(const id of [coordinator,writer])await f.c.command('ownership',{target:id});await f.c.command('finish_pause',{target:f.seed.run.id});await f.c.command('release',{target:writer});
 return {f,candidate,work,r,target,writer,completion,observer,complete:()=>f.c.command('complete',{target:f.seed.run.id,key:completion.id})};
}
