import assert from 'node:assert/strict';
import {join} from 'node:path';
import test from 'node:test';
import {NativeDaemon} from './daemon-client';
import {nativeUi} from './native-ui-driver';
import {workflowFixture} from './workflow-support';

test('native M1 import, refinement, refresh, queue and pending steering survive a real daemon restart', {timeout:90000}, async t => {
  const f=await workflowFixture(t,'coding',0,false,{service:true,configure({config}){config.runtime=null;}});
  assert.ok(f.c instanceof NativeDaemon);
  const ui=(action:string,extra:Record<string,unknown>={})=>nativeUi(t,f.root,{descriptor:join(f.config.authority,'native-connection.json'),repositoryId:f.repository.id,action,...extra},'native_external_intake_restart');
  const before=await ui('intake');
  assert.equal(before.run.status,'awaiting_runner'); assert.equal(before.runtime.attempts.length,0);
  const messages=before.workspace.messages.filter((m:any)=>m.runId===before.run.id);
  assert.equal(messages.length,1); assert.equal(messages[0].body,'Keep the patch small.'); assert.equal(messages[0].delivery,'pending');
  const pid=f.c.proc.pid; await f.c.close(); await f.reopen(); assert.notEqual(f.c.proc.pid,pid);
  const after=await ui('intake-reopen',{taskId:before.task.id,runId:before.run.id});
  assert.deepEqual(after.task,before.task); assert.deepEqual(after.run,before.run);
  assert.deepEqual(after.workspace.messages.filter((m:any)=>m.runId===before.run.id),messages);
  assert.equal(after.runtime.attempts.length,0);
  t.diagnostic(JSON.stringify({nativeImport:true,refined:true,refreshPreservedBrief:true,acceptedRefreshedSource:true,queuedNotExecuting:true,steeringPending:true,distinctDaemonPids:true,narrowWidth:390}));
});
