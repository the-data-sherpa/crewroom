/** RTE-01/02 binding breadth. Pure native policy, not provider availability or live dispatch. */
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {readFileSync} from 'node:fs';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
const catalog=JSON.parse(readFileSync('docs/testing/fixtures/omp-model-catalog-2026-09-26.json','utf8'));
const profiles=JSON.parse(readFileSync('docs/testing/model-profiles.json','utf8'));
const operations:any[]=[], labels:any[]=[];
function add(roleId:string,model:any,effort:string,profile?:string){
 const role={...defaultRoles[0],id:roleId,model:model.selector,effort};
 const capability={provider:model.provider,id:model.id,api:model.api,inputs:model.declaredInput,efforts:[...new Set(['off',...model.declaredThinkingEfforts])],tools:null};
 operations.push({operation:'resolve',role,catalog:[capability],policy:{},requirements:{}});
 labels.push({roleId,selector:model.selector,effort,profile:profile??'complete-catalog'});
}
for(const role of profiles.roles)for(const model of catalog.models)add(role.id,model,'off');
assert.equal(operations.length,950);
for(const profile of profiles.profiles)for(const assignment of profile.assignments){
 const selection=profiles.modelPool[assignment.modelKey],model=catalog.models.find((m:any)=>m.selector===selection.selector);
 assert.ok(model);add(assignment.roleId,model,selection.effort??'off',profile.id);
}
const result=spawnSync('target/debug/examples/policy-oracle',[],{input:JSON.stringify(operations),encoding:'utf8',maxBuffer:8*1024*1024,timeout:15000});
assert.equal(result.status,0,result.stderr);const actual=JSON.parse(result.stdout);assert.equal(actual.length,1007);
for(const [i,op] of operations.entries()){
 const expected=JSON.parse(JSON.stringify(resolveAssignment(op.role,op.catalog,op.policy,op.requirements)));
 assert.deepEqual(actual[i],{value:expected},JSON.stringify(labels[i]));
 assert.equal(actual[i].value.resolved.selector,labels[i].selector);
 assert.equal(actual[i].value.resolved.provider,op.catalog[0].provider);
 assert.equal(actual[i].value.resolved.id,op.catalog[0].id);
 assert.equal(actual[i].value.resolved.api,op.catalog[0].api);
 assert.equal(actual[i].value.resolved.tools,null,'Binding must not imply tool capability.');
}
console.log(JSON.stringify({passed:true,catalogBindings:950,namedProfileBindings:57,profiles:profiles.profiles.map((p:any)=>p.id),inputSha256:createHash('sha256').update(JSON.stringify(operations)).digest('hex'),resultSha256:createHash('sha256').update(JSON.stringify(actual)).digest('hex'),bindings:labels,boundary:'Pure Rust/retained policy comparison of every captured role/model identity and profile binding. No provider calls or execution eligibility claim.'},null,2));
