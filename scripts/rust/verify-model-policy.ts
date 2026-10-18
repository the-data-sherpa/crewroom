import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { resolveAssignment, resolveRuntimeFallback, verifyAssignmentIdentity, isolatedModelSettings, configureAssignment } from '../../server/runtime/model-policy';
import { defaultRoles } from '../../server/roles';
const base=[{provider:'fixture',id:'code',api:'openai-responses',inputs:['text'],efforts:['off','low','high'],tools:true},
  {provider:'fixture',id:'plain',api:'anthropic-messages',inputs:['text'],efforts:['off'],tools:false},
  {provider:'fixture',id:'alternate',api:'google-generative-ai',inputs:['text','image'],efforts:['off','low','high'],tools:null}];
const role={...defaultRoles[1],model:'fixture/code',effort:'low' as const};
const operations:any[]=[]; const add=(operation:string,extra:object)=>operations.push({operation,role,catalog:base,policy:{},requirements:{},...extra});
for(const model of ['fixture/code','fixture/plain','fixture/absent','']) for(const effort of ['default','off','minimal','low','medium','high','xhigh','max'])
 for(const policy of [{},{default:{model:'fixture/code',effort:'high'}},{fallbacks:{'fixture/absent':[{model:'fixture/missing',effort:'low'},{model:'fixture/alternate',effort:'low'},{model:'fixture/code',effort:'low'}]}}])
  for(const requirements of [{},{tools:true},{inputs:['image']},{allowedModels:['fixture/code']},{allowedModels:[]}]) add('resolve',{role:{...role,model,effort},policy,requirements});
for(const catalog of [[],[...base,base[0]],[...base,{...base[0],efforts:['off']}],base.map(v=>({...v,tools:null})),base.map(v=>({...v,inputs:[]}))])
 for(const enabled of [false,true]) add('resolve',{role:{...role,enabled},catalog,requirements:{tools:true,inputs:['text']}});
const plan=resolveAssignment(role,base,{},{});
const state={model:{provider:'fixture',id:'code',api:'openai-responses',name:'ignored'},thinkingLevel:'low',sessionId:'session',isStreaming:false,autoCompactionEnabled:false,extra:'ignored'};
add('identity',{plan,input:state});
for(const key of Object.keys(state))for(const value of [undefined,null,'',false,true,0,{},[]])add('identity',{plan,input:{...state,[key]:value}});
for(const effort of ['off','low','high']) {
 const changed=resolveAssignment({...role,effort:effort as 'off'|'low'|'high'},base,{},{});
 add('settings',{plan:changed});
 for(const thinkingLevel of [undefined,null,'off','low','high']) add('identity',{plan:changed,input:{...state,thinkingLevel}});
}
for(const changes of [{provider:'other'},{id:'plain'},{api:'wrong'}])add('identity',{plan,input:{...state,model:{...state.model,...changes}}});
for(const requirements of [{},{tools:true},{allowedModels:['fixture/code']},{inputs:['image']}])
 for(const tried of [[],['fixture/code'],['fixture/alternate','fixture/plain']])
 for(const targets of [[],[{model:'fixture/alternate',effort:'low'}],[{model:'fixture/missing',effort:'low'},{model:'fixture/plain',effort:'off'},{model:'fixture/alternate',effort:'medium'},{model:'fixture/alternate',effort:'high'}]])
  add('fallback',{current:plan,requirements,tried,policy:{fallbacks:{'fixture/code':targets}}});
add('fallback',{current:plan,tried:[],catalog:[...base,base[0]]});
const replies={get_available_models:{value:{models:base}},set_model:{value:{}},get_available_thinking_levels:{value:{levels:['off','low','high']}},set_thinking_level:{value:{}},set_auto_compaction:{value:{}},set_auto_retry:{value:{}},get_state:{value:state}};
add('configure',{plan,replies});
for(const key of Object.keys(replies))add('configure',{plan,replies:{...replies,[key]:{error:'fixture failure at '+key}}});
for(const value of [null,{}, {models:[]}, {models:[{provider:'fixture',id:'code',api:'wrong'}]}, {models:[null]}])add('configure',{plan,replies:{...replies,get_available_models:{value}}});
for(const value of [null,{}, {levels:[]},{levels:['off']},{levels:[0]}])add('configure',{plan,replies:{...replies,get_available_thinking_levels:{value}}});
for(const value of [null,{}, {...state,isStreaming:true},{...state,autoCompactionEnabled:true},{...state,thinkingLevel:'high'}])add('configure',{plan,replies:{...replies,get_state:{value}}});
add('configure',{plan:resolveAssignment({...role,effort:'off'},base,{},{}),replies:{...replies,get_available_thinking_levels:{value:{levels:[]}},get_state:{value:{...state,thinkingLevel:null}}}});
// Strip undefined exactly at the JSON boundary sent to Rust.
const inputs=JSON.parse(JSON.stringify(operations));
const expected=await Promise.all(inputs.map(async(v:any)=>{const calls:any[]=[];try{
 if(v.operation==='configure') {
   const value=await configureAssignment({async command(type,args={}){calls.push({type,args});const r=v.replies[type];if(r?.error)throw new Error(r.error);return r?.value??null;}},v.plan);
   return {value,calls};
 }
 let value:any;
 if(v.operation==='resolve')value=resolveAssignment(v.role,v.catalog,v.policy,v.requirements);
 else if(v.operation==='fallback')value=resolveRuntimeFallback(v.role,v.current,v.catalog,v.policy,v.requirements,v.tried)??null;
 else if(v.operation==='identity')value=verifyAssignmentIdentity(v.plan,v.input);
 else value=isolatedModelSettings(v.plan);
 return JSON.parse(JSON.stringify({value}));
}catch(error){const last=calls.at(-1)?.type;return {error:error instanceof Error && error.name==='ZodError'?(last==='get_available_models'?'Invalid runner catalog.':last==='get_available_thinking_levels'?'Invalid runner effort catalog.':'Invalid runner identity.'):String(error instanceof Error?error.message:error),...(v.operation==='configure'?{calls}:{})};}}));
const child=spawnSync('target/debug/examples/policy-oracle',[],{input:JSON.stringify(inputs),encoding:'utf8',timeout:15000,maxBuffer:8*1024*1024});
assert.equal(child.status,0,child.stderr);const actual=JSON.parse(child.stdout);assert.equal(actual.length,expected.length);
for(let i=0;i<expected.length;i++)assert.deepEqual(actual[i],expected[i],JSON.stringify({case:i,input:inputs[i]}));
console.log(JSON.stringify({status:'passed',comparisons:inputs.length,corpusSha256:createHash('sha256').update(JSON.stringify(inputs)).digest('hex'),counts:Object.fromEntries(['resolve','fallback','identity','settings','configure'].map(k=>[k,inputs.filter((v:any)=>v.operation===k).length])),boundary:'Pure retained policy on JSON inputs; schema identity errors normalized to Invalid runner identity. Actual RPC, provider enforcement, model availability and workflow admission are separate checks.'},null,2));
