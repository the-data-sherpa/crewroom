/** Native GPUI controls over the production daemon, SCRIPTED OMP and Mail.
 * Real process ownership/contained checks; no live model or OS-input claim.
 */
import assert from 'node:assert/strict';
import {copyFileSync,existsSync,mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {join,resolve} from 'node:path';
import test from 'node:test';
import {NativeDaemon} from './daemon-client';
import {nativeUi} from './native-ui-driver';
import {workflowFixture} from './workflow-support';

const imageReceipt=process.env.CREWROOM_FOR14_CHECK_IMAGE_RECEIPT??resolve('.crewroom/test-results/for14/check-image-v1-final/image-receipt.json');
const skip=!existsSync(imageReceipt)?'Build the protected FOR-14 image first.':false;
test('FOR-14 native role/principle/playbook comparison, controls, exact adoption and ordinary saves use the production daemon',{timeout:360000,skip},async t=>{
 const parent=process.env.CREWROOM_FOR14_NATIVE_ROOT;if(parent)mkdirSync(parent,{recursive:true,mode:0o700});
 const f=await workflowFixture(t,'coding',0,false,{service:true,...(parent?{parent}:{}),cleanup:()=>{},configure:({root,config})=>{
  const executable=join(root,'scripted-omp-identity');copyFileSync('/usr/bin/false',executable);
  config.runtime.ompBinary=executable;config.runtime.evaluationMode='scripted';
  config.runtime.evaluationImage=JSON.parse(readFileSync(imageReceipt,'utf8')).image;
 }});
 assert.ok(f.c instanceof NativeDaemon);const c=f.c;
 const principle=await c.request('/api/principles',{id:'for14-native',name:'Verification limits',text:'Record actual verification limits.',revision:0});
 const roles=(await c.request('/api/workspace')).roles;const implementer=roles.find((role:any)=>role.id==='implementer');
 await c.request('/api/roles/implementer',{...implementer,revision:implementer.revision??1,principles:[principle.id]});
 const playbook=await c.request('/api/playbooks',{id:'for14-native',name:'Native comparison checklist',description:'Use for the requested local change.',steps:[{id:'step-1',title:'Convey the accepted task.'},{id:'step-2',title:'Receive the specialist handoff.'}],revision:0});
 const before=await c.request('/api/workspace');const results:any[]=[];
 for(const [kind,targetId] of [['role','implementer'],['principle',principle.id],['playbook',playbook.id]]){
  const result=await nativeUi(t,f.root,{descriptor:join(f.config.authority,'native-connection.json'),kind,targetId,action:`compare-${kind}`},'native_external_instruction_comparison');
  const comparison=await c.request(`/api/comparisons/${result.retained.id}`);
  assert.equal(comparison.manifestHash,result.started.manifestHash);assert.deepEqual(comparison.manifest,result.started.manifest);
  assert.equal(comparison.state,'cancelled');assert.equal(comparison.cells.length,16);assert.equal(comparison.coverage.observed,0);
  assert.equal(comparison.adoption.proposedHash,comparison.manifest.target.proposedHash);
  assert.equal(comparison.adoption.adoptedRevision,comparison.manifest.target.baseRevision+1);
  const catalog={role:'roles',principle:'principles',playbook:'playbooks'}[kind]!;
  const saved=(await c.request('/api/workspace'))[catalog].find((row:any)=>row.id===targetId);
  assert.equal(saved.revision,comparison.manifest.target.baseRevision+2,'Ordinary save is independent of comparison evidence');
  if(kind==='role'){
   const cell=comparison.cells.find((cell:any)=>cell.runId);assert.ok(cell);
   const runtime=await c.request(`/api/runs/${cell.runId}/runtime`);
   assert.equal(runtime.assignments.length,2);assert.equal(runtime.attempts.length,2);
   assert.ok(runtime.attempts.every((attempt:any)=>attempt.hostPid&&attempt.ompPid&&attempt.ownership==='stopped'));
   assert.equal(runtime.assignments.find((assignment:any)=>!assignment.coordinator).role.instructions,comparison.manifest.target.current.instructions,'Later saves do not rewrite admitted assignments');
  }
  results.push({kind,comparisonId:comparison.id,manifestHash:comparison.manifestHash,baseRevision:comparison.manifest.target.baseRevision,adoptedRevision:comparison.adoption.adoptedRevision,ordinarySavedRevision:saved.revision,coverage:comparison.coverage,processAttempts:comparison.cells.flatMap((cell:any)=>cell.attempts??[]).map((attempt:any)=>({id:attempt.id,hostPid:attempt.hostPid,runnerPid:attempt.ompPid,ownership:attempt.ownership}))});
 }
 const after=await c.request('/api/workspace');assert.deepEqual(after.tasks,before.tasks);assert.equal((await c.request('/api/comparisons')).length,3);
 const receipt={mode:'scripted',surface:'headless GPUI with production authenticated HTTP/WS daemon',preflightCreateHistoryAllKinds:true,nativeStartPauseCancel:true,exactAdoptionAllKinds:true,ordinarySavesAllKinds:true,admittedAssignmentsUnchanged:true,results};
 writeFileSync(join(f.root,'for14-native-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});t.diagnostic(JSON.stringify(receipt));
});
