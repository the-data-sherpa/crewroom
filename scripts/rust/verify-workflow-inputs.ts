/** Pure retained workflow contracts. The stub store supplies captured records;
 * admission reservations, live providers, processes and delivery are not tested here. */
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {parseTeam} from '../../server/run-launch';
import {presentBriefs,snapshotBriefs} from '../../server/runtime/brief';
import {RuntimeSupervisor} from '../../server/runtime/supervisor';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
import {hostInputLimit} from '../../server/runtime/types';

const id=(n:number)=>'00000000-0000-4000-8000-'+String(n).padStart(12,'0');
const config={root:'/tmp/crewroom-inputs',socket:'/tmp/crewroom-inputs/tmux.sock',ompBinary:'/fixture/omp',
  mail:{url:'http://127.0.0.1:4319/mcp/',token:'synthetic-mail-private',projectRoot:'/tmp/crewroom-mail',retentionSeconds:86400,atcDisabled:true},
  catalog:[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['off','low','high'],tools:true}],
  policy:{default:{model:'fixture/model',effort:'low'}},
  models:{providers:{fixture:{baseUrl:'https://provider.invalid/{agent}/v1',apiKey:'synthetic-upstream-private',api:'openai-responses',headers:{Authorization:'synthetic-header'},models:[{id:'model',contextWindow:1050000,maxTokens:128000}]}}}};
const roles=defaultRoles.map(r=>({...r,model:'fixture/model',effort:'low' as const}));
const coordinator=roles.find(r=>r.id==='coordinator')!;
const run={id:id(1),purpose:'coding',status:'awaiting_runner',taskRevision:2,taskSnapshot:{source:'local',title:'Captured task',brief:'Preserve scope',criteria:['First criterion','Second criterion']}};
const member={name:'Coordinator',roleId:coordinator.id,coordinator:true,input:'Inspect accepted requirements',criteria:[],dependsOn:[]};
const input={team:[member]};
const operations:any[]=[];const nativeOnly=new Map<number,(r:any)=>void>();
const add=(op:string,fields:Record<string,unknown>={})=>{operations.push({op,...fields});return operations.length-1;};
const special=(op:string,fields:Record<string,unknown>,check:(r:any)=>void)=>nativeOnly.set(add(op,fields),check);
const base={config,run,roles,checks:[],input};

for(const changed of [input,{},null,{...input,repository:{mode:'new'}},{...input,extra:true},{team:[]},{team:Array(5).fill(member)},
  {team:[member,{...member,name:'Second'}]},{team:[{...member,coordinator:false}]},{team:[{...member,coordinator:undefined}]},
  {team:[{...member,coordinator:'true'}]},{team:[member,{...member,coordinator:false}]}
])add('team',{input:changed});
for(const name of ['A','A_9-z','a'.repeat(61),'a'.repeat(62),'9Name','Bad name','','💡'])add('team',{input:{team:[{...member,name}]}});
for(const body of ['', 'x', 'x'.repeat(40000),'x'.repeat(40001),'😀'.repeat(20001),'😀'.repeat(40000),'😀'.repeat(40001)])add('team',{input:{team:[{...member,input:body}]}});
for(const criterion of [{index:0,obligation:'  cover this  ',checkIds:['unit','unit']},{index:-1,obligation:'x'},{index:0.5,obligation:'x'},{index:0,obligation:''},{index:0,obligation:'x',unknown:true}])add('team',{input:{team:[{...member,criteria:[criterion]}]}});
for(const purpose of ['coding','research','explore','grill']) for(const role of roles) {
 const specialist={name:'Specialist',roleId:role.id,coordinator:false,input:'Inspect assigned evidence',criteria:[],dependsOn:['Coordinator']};
 for(const criteria of [[],[{index:0,obligation:'Confirm the first criterion',checkIds:[]}]])add('prepare',{...base,run:{...run,purpose},input:{team:[member,{...specialist,criteria}]}});
}
for(const changed of [{sourceConflict:'Adopt the changed requirements.'},{purpose:'explore'},{purpose:'grill'},{purpose:'research'}])add('prepare',{...base,run:{...run,...changed}});
for(const changedRoles of [[],roles.map(r=>({...r,enabled:false})),roles.map(r=>({...r,model:'missing/model'}))])add('prepare',{...base,roles:changedRoles});
for(const criteria of [[{index:0,obligation:'Cover it',checkIds:['unit','unit']}],[{index:2,obligation:'Missing',checkIds:[]}],[{index:0,obligation:'Cover it',checkIds:['absent']}],[{index:0,obligation:'One',checkIds:[]},{index:0,obligation:'Two',checkIds:[]}]])add('prepare',{...base,checks:['unit'],input:{team:[{...member,criteria}]}});
for(const deps of [['Coordinator'],['Missing'],[],['Specialist']]) {
 const specialist={...member,name:'Specialist',coordinator:false,dependsOn:['Coordinator']};
 add('prepare',{...base,input:{team:[{...member,dependsOn:deps},specialist]}});
}
for(const purpose of ['explore','grill','coding'])for(const selected of [roles,roles.filter(r=>r.id!=='coordinator'),roles.map(r=>({...r,enabled:false}))])add('refinement',{...base,run:{...run,purpose},roles:selected});
const owner=(v:any)=>{
 const target=Object.create(RuntimeSupervisor.prototype);
 Object.assign(target,{config:v.config??config,hosts:new Map(),closing:false,roleCatalog:()=>v.roles??roles,store:{
  getRun:()=>v.run??run,repositoryWork:()=>v.work,codingRecord:()=>v.coding,publication:()=>v.publication,configuredCheckIds:()=>v.checks??[],
  validateInsertionTeam:()=>{},steering:()=>v.steering??[],deliveryNotices:()=>v.notices??[],
  runtime:()=>({messages:v.messages??[]}),agentEventsOfType:(_:string,types:string[])=>(v.events??[]).filter((e:any)=>types.includes(e.event.type))
 }});
 return target;
};
const call=(target:any,name:string,...args:any[])=>Reflect.apply(Reflect.get(target,name),target,args);

const own={id:id(2),currentAttemptId:id(3),name:'Coordinator',coordinator:true,role:coordinator,model:resolveAssignment(coordinator,config.catalog,config.policy,{}),brief:{criteria:[],dependsOn:[]}};
const team=[own,{id:id(4),name:'RetiredReferenced',role:roles[1],retiredAt:'2026-10-01',brief:{criteria:[],dependsOn:[]}},
 {id:id(5),name:'RetiredUnreferenced',role:roles[1],retiredAt:'2026-10-01',brief:{criteria:[],dependsOn:[]}}];
for(const deps of [[],['RetiredReferenced']])add('present',{own:{...own,brief:{...own.brief,dependsOn:deps}},team:[{...own,brief:{...own.brief,dependsOn:deps}},...team.slice(1)]});
add('present',{own:{name:'Legacy'},team:[{name:'Legacy',role:{name:'Retained'}}]});
const coding={edits:[{id:id(20),input:{path:'file.txt',expectedHash:'hash',content:'OMITTED EDIT BODY',edits:[{oldText:'x',newText:'y'}]},state:'applied'}],
 checks:[{id:id(21),definition:{id:'unit',command:'OMITTED CHECK COMMAND'},output:{sha256:'output-hash',path:'private-output'},source:{sha256:'source-hash',path:'private-source'},passed:true}],
 checkOperations:[{id:id(22),definition:{id:'unit'},workspace:'private-workspace',source:{path:'private-source'},state:'prepared'}]};
const work={instructions:[{path:'AGENTS.md',sha256:'instruction-hash',content:'OMITTED INSTRUCTION BODY'}],
 checkpoints:[1,2,3,4,5],refreshes:[{requestId:id(30),state:'conflicted',resolutionWorkspace:{state:'conflicted',toHead:'head',path:'private-conflict',promotionProcess:{token:'private-process'}}}]};
for(const record of [undefined,{checks:[]},coding])add('coding',{coding:record});
for(const n of [0,1,5,6,10,11,25])for(const body of ['short','a'.repeat(850),'😀'.repeat(425)]) {
 const steering=Array.from({length:n},(_,i)=>({id:id(i+100),body,delivery:i%4===0?'pending':i%4===1?'not_delivered':'applied',dispositionReason:'Historical closure'}));
 const messages=Array.from({length:n},(_,i)=>({id:id(i+200),body,disposition:i%3===0?'accepted':'received'}));
 const notices=Array.from({length:n},(_,i)=>({id:id(i+300),disposition:i%3===0?'pending':'received',payload:{kind:'fixture'}}));
 add('context',{coding,work,publication:{id:id(40),description:'OMITTED DESCRIPTION',feedback:{body:'UNPINNED FEEDBACK'},observation:{opaque:'OMITTED'},state:'review'},steering,messages,notices});
}
add('context',{});
add('context',{work:{instructions:[],checkpoints:[],refreshes:[{requestId:id(31),state:'conflicted',conflicts:['file.txt']}]}});
const split='a'.repeat(299)+'😀'+'z'.repeat(20);
special('context',{steering:Array.from({length:6},(_,i)=>({id:id(i+700),body:split,delivery:'applied'}))},r=>{
 assert.equal(r.value.steering[0].body,'a'.repeat(299)+'… [22 characters omitted; reconciled earlier]');
 assert.equal(r.value.steering[5].body,split);
});
for(const errorMessage of ['prompt is too long','CONTEXT_LENGTH exceeded','context-window overflow','context length','maximum context','too many tokens','authorization failed','context  length'])for(const stopReason of ['error','stop']) {
 const events=[{attemptId:'old',event:{type:'message_end',message:{role:'assistant',stopReason,errorMessage}}}];
 add('session',{attempt:'old',events});add('session',{attempt:'new',events});
 add('session',{attempt:'old',events:[...events,{attemptId:'old',event:{type:'message_end',message:{role:'assistant',stopReason:'stop'}}}]});
}
for(const code of ['input_budget','other'])for(const recovery of ['fresh_session','retry'])add('session',{attempt:'old',events:[{attemptId:'old',event:{type:'model_policy_rejected',code,recovery}}]});
add('history',{attempt:'old',inputs:[{id:'initial:old',message:'Preserve unfinished',disposition:'error'},{id:'peer',message:'Remove completed',disposition:'completed'},{id:'continuation:old',message:'Remove previous context',disposition:'error'}]});
for(const n of [1,1000,74000,75000,149950,150000])for(const text of ['x','😀','"'])add('input',{id:'continuation:fixture',message:{body:text.repeat(n)}});

const source=readFileSync('server/runtime/supervisor.ts','utf8');
const start=source.indexOf('        const run = this.store.getRun(runId);\n        const researchPrompt');
const end=source.indexOf('        const command = [...(this.config.launchPrefix',start);
assert.ok(start>=0&&end>start);
const retainedPrompt=new Function('assignment','attempt','assignments','work','purpose','team','root','runnerCwd','model','refinement','presentBriefs','join','runId',source.slice(start,end)+'\nreturn args;');
const attempt={id:id(3),runId:run.id,assignmentId:own.id,input:'Captured assignment'};
const environment={HOME:'/Users/fixture',PATH:'/fixture/bin'};
const hostBase={config,run,assignment:own,attempt,team:[own],providerToken:'synthetic-gateway-private',environment,jira:true,publication:true};
for(const purpose of ['coding','explore','grill','research'])for(const source of ['local','jira'])add('host',{...hostBase,run:{...run,purpose,taskSnapshot:{...run.taskSnapshot,source}}});
add('host',{...hostBase,team});
add('host',{...hostBase,config:{...config,launchPrefix:['/fixture/guard','--offline']}});
add('host',{...hostBase,run:{...run,purpose:'research',researchSource:{commit:'pinned-source'}},assignment:{...own,role:roles.find(r=>r.id==='delivery-reporter')!}});
add('host',{...hostBase,assignment:{...own,projectGuidance:{version:3,text:'Accepted project guidance'}}});
const writer={...own,id:id(4),currentAttemptId:id(6),name:'Specialist',coordinator:false,role:roles.find(r=>r.id==='implementer')!};
add('host',{...hostBase,assignment:writer,attempt:{...attempt,id:id(6),assignmentId:writer.id},team:[own,writer],work:{...work,state:'ready',path:'/fixture/work',branch:'crewroom/run',base:'base',repository:{checks:[]}}});
const previous={id:id(7),assignmentId:own.id,runnerCwd:'/tmp/crewroom-inputs/assignments/'+own.id,sessionDirectory:'/tmp/crewroom-inputs/'+id(7)+'/sessions',sessionId:'old-session'};
for(const resumeSession of [false,true])add('host',{...hostBase,previous,resumeSession,attempt:{...attempt,previousAttemptId:previous.id}});
add('host',{...hostBase,previous,resumeSession:true,attempt:{...attempt,previousAttemptId:previous.id,model:{...own.model,resolved:{...own.model.resolved,id:'other',selector:'fixture/other'}}}});
for(const patch of [{mail:{...config.mail,atcDisabled:false}},{mail:{...config.mail,retentionSeconds:0}},{hostPorts:[]},{hostPorts:[12,12]},{hostPorts:[0]},{hostPorts:[65536]},{hostPorts:[12.5]},{tools:[{id:'unsafe'}]}]) {
 special('config',{config:{...config,...patch}},r=>assert.ok(r.error,'Invalid runtime configuration admitted'));
}
for(const patch of [{root:'relative'},{socket:'/outside/tmux.sock'},{ompBinary:'relative'},{launchPrefix:['relative']},{commandEngine:{socket:'relative'}}])special('config',{config:{...config,...patch}},r=>assert.ok(r.error));
special('host',{...hostBase,attempt:{...attempt,assignmentId:id(99)}},r=>assert.ok(r.error));
special('host',{...hostBase,previous:{...previous,runnerCwd:'/outside'},attempt:{...attempt,previousAttemptId:previous.id}},r=>assert.ok(r.error));
special('host',{...hostBase,config:{...config,hostPorts:[4501]},port:4502},r=>assert.ok(r.error));
special('host',{...hostBase,config:{...config,hostPorts:[4501]},port:4501},r=>assert.equal(r.value.port,4501));
for(const url of ['file:///tmp/mail','https://mail.invalid/mcp/','http://127.0.0.1:4319/?token=private','http://user:private@127.0.0.1:4319/'])special('config',{config:{...config,mail:{...config.mail,url}}},r=>assert.ok(r.error));
special('host',{...hostBase,work:{...work,state:'ready',repository:{checks:[{required:true}]}}},r=>assert.ok(r.error));
special('host',{...hostBase,previous:{...previous,sessionDirectory:'/outside/sessions'},attempt:{...attempt,previousAttemptId:previous.id},resumeSession:true},r=>assert.ok(r.error));
special('host',{...hostBase,providerToken:''},r=>assert.ok(r.error));

const expected=(v:any)=>{
 const target=owner(v);
 switch(v.op){
  case 'team':return parseTeam(v.input);
  case 'prepare':return call(target,'preflightStart',run.id,v.input,v.roles);
  case 'refinement':target.start=(_:string,input:any)=>input;return call(target,'startRefinement',run.id,v.roles);
  case 'present':return presentBriefs(v.own,v.team);
  case 'coding':return call(target,'codingContext',run.id)??null;
  case 'context':return call(target,'continuationContext',run.id);
  case 'session':return call(target,'sessionNeedsReplacement',run.id,v.attempt);
  case 'history':return v.inputs.map((i:any)=>({...i,message:i.id.startsWith('continuation:')||i.disposition==='completed'?undefined:i.message,attemptId:v.attempt}));
  case 'input':{
   const body={op:'input',id:v.id,message:JSON.stringify(v.message)};
   if(JSON.stringify(body).length>hostInputLimit)throw new Error('Input too large');return body;
  }
  case 'host':return null;
  default:throw new Error('Unexpected oracle operation');
 }
};
const inputs=JSON.parse(JSON.stringify(operations));
const oracle=inputs.map((v:any,i:number)=>{if(nativeOnly.has(i)||v.op==='host')return null;try{return {value:JSON.parse(JSON.stringify(expected(v)))}}catch(error){return {error:true,reason:String(error)}}});
const child=spawnSync('target/debug/examples/workflow-inputs',[],{input:JSON.stringify(inputs),encoding:'utf8',timeout:30000,maxBuffer:96*1024*1024});
assert.equal(child.status,0,child.stderr);const actual=JSON.parse(child.stdout);assert.equal(actual.length,inputs.length);
let comparisons=0,hostCases=0;
for(let i=0;i<inputs.length;i++){
 const v=inputs[i],r=actual[i],custom=nativeOnly.get(i);
 if(custom){custom(r);continue;}
 if(v.op==='host'){
  assert.ok(!r.error,'Host input case '+i+': '+r.error);const c=r.value,purpose=v.run.purpose;
  const assignments=v.team,active=assignments.filter((a:any)=>!a.retiredAt),model=v.attempt.model??v.assignment.model;
  const expectedCwd=v.previous?.runnerCwd??(v.previous?join(v.config.root,v.previous.id):join(v.config.root,'assignments',v.assignment.id));
  assert.equal(c.runnerCwd,expectedCwd);
  const target={config:v.config,jiraDelivery:v.jira?{}:undefined,store:{getRun:()=>v.run}};
  const args=retainedPrompt.call(target,v.assignment,v.attempt,assignments,v.work,purpose,active,join(v.config.root,v.attempt.id),expectedCwd,model,['explore','grill'].includes(purpose),presentBriefs,join,v.run.id);
  assert.deepEqual([c.command,...c.args],[...(v.config.launchPrefix??[]),...args],'Prompt/isolation case '+i);
  assert.equal(c.providerToken,v.providerToken);assert.notEqual(c.providerToken,c.upstreamProvider.apiKey);
  assert.equal(c.upstreamProvider.baseUrl,v.config.models.providers[model.resolved.provider].baseUrl.replaceAll('{agent}',encodeURIComponent(v.assignment.name)));
  assert.equal(c.env.GIT_CONFIG_GLOBAL,'/dev/null');assert.equal(c.env.GIT_CONFIG_NOSYSTEM,'1');
  assert.equal(c.env.PI_CODING_AGENT_DIR,join(v.config.root,v.attempt.id,'config'));
  assert.equal(c.budgetRoot,join(v.config.root,'budgets',v.assignment.id));
  assert.equal(c.env.HOME,v.environment.HOME);assert.equal(c.env.PATH,v.environment.PATH);
  assert.equal(c.coding,!!v.work);assert.equal(c.research,purpose==='research');assert.equal(c.researchRepository,!!v.run.researchSource);
  assert.equal(c.refinement,['explore','grill'].includes(purpose));assert.equal(c.publication,purpose==='coding'&&v.publication);
  assert.equal(c.jira,!['explore','grill'].includes(purpose)&&v.jira);
  assert.equal(c.reporter,v.assignment.role.id==='delivery-reporter'&&!v.assignment.coordinator);
  assert.equal(c.researchCoordinatorReporter,purpose==='research'&&v.assignment.coordinator&&!active.some((a:any)=>a.role.id==='delivery-reporter'));
  assert.deepEqual(c.recipients,active.map((a:any)=>a.name));assert.deepEqual(c.tools,[]);
  assert.ok(!('token' in c)&&!('backend' in c),'Host authority must be bound later by HostManager');
  if(v.previous){
   assert.equal(c.previousAttemptId,v.previous.id);
   assert.equal(c.resumeSessionDir,v.resumeSession&&(v.previous.model??v.assignment.model).resolved.selector===model.resolved.selector?v.previous.sessionDirectory:undefined);
  }
  hostCases++;continue;
 }
 if(oracle[i]?.error)assert.ok(r.error,'Expected refusal at '+i+' '+v.op+' '+oracle[i]?.reason+' keys='+Object.keys(v).join(','));
 else assert.deepEqual(r,oracle[i],'Pure retained comparison '+i+' '+v.op);
 comparisons++;
}
console.log(JSON.stringify({passed:true,comparisons,hostPromptAndConfigurationCases:hostCases,additionalNativeCases:nativeOnly.size,total:inputs.length,
 corpusSha256:createHash('sha256').update(JSON.stringify(inputs)).digest('hex'),
 retainedSupervisorSha256:createHash('sha256').update(source).digest('hex'),
 counts:Object.fromEntries([...new Set(inputs.map((v:any)=>v.op))].map(op=>[op,inputs.filter((v:any)=>v.op===op).length])),
 boundary:'Pure captured request/configuration/context contracts. No durable admission, native workflow dispatch, live providers or process/Mail delivery. One documented abbreviation case keeps a complete Unicode scalar where JS slices a surrogate pair.'
},null,2));
