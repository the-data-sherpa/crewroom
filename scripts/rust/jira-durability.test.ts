import assert from 'node:assert/strict';
import {DatabaseSync} from 'node:sqlite';
import {test} from 'node:test';
import {body,jiraFixture} from './jira-workflow-fixture';

test('native Jira SQL failures before dispatch and after effect retain truthful replay states',async t=>{
 for(const state of ['sending','confirmed'])await t.test(state,async t=>{
  let posts=0;const comments:any[]=[];const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){comments.push({id:String(++posts),body:(await body(req)).body});res.end(JSON.stringify({id:String(posts)}));}else res.end(JSON.stringify({comments,startAt:0,total:comments.length}));});
  await f.report({kind:'milestone',eventKey:'durability',body:'Retain exact outcome.'});const db=new DatabaseSync(f.database);db.exec(`CREATE TRIGGER fail_update BEFORE UPDATE ON jira_updates WHEN json_extract(NEW.data,'$.state') = '${state}' BEGIN SELECT RAISE(ABORT,'fixture durable write failed'); END;`);db.close();
  await assert.rejects(f.flush(),/fixture durable write failed/);assert.equal(posts,state==='sending'?0:1);assert.equal((await f.updates())[0].state,state==='sending'?'pending':'sending');
  const repair=new DatabaseSync(f.database);repair.exec('DROP TRIGGER fail_update');repair.close();await f.reopen();await f.flush();await f.flush();assert.equal(posts,1);assert.equal((await f.updates())[0].state,'confirmed');
 });
});
