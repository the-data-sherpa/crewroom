import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { ObservationStream, compactObservation } from '../../server/runtime/observation';
import { PaneFormatter, summarizeArguments } from '../../server/runtime/pane-format';
const inputs:any[]=[];
for(const phase of [undefined,null,'text_delta',123,{},'x'.repeat(90)])for(const index of [undefined,null,0,1,-0,-1,1.2,9007199254740991,9007199254740992,'1',true])
 for(const messageId of [undefined,null,'id','a'.repeat(250),{},8])inputs.push({op:'compact',event:{type:'message_update',phase,contentIndex:index,messageId,assistantMessageEvent:{type:'toolcall_delta',contentIndex:2},message:{content:'never retained'}}});
for(let now=0;now<12000;now++)inputs.push({op:'observe',now,event:{type:'message_update',messageId:'stream',assistantMessageEvent:{type:'text_delta',delta:'large'}}});
for(const messageId of [undefined,null,{},[],1,1,'next','next'])inputs.push({op:'observe',now:12001,event:{type:'message_update',messageId}});
inputs.push({op:'observe',now:12002,event:{type:'host_tool_call',arguments:{content:'complete'}}});
const kinds=['host_ready','session_restore_inspected','agent_start','message_end','host_tool_call','tool_execution_start','tool_execution_end','tool_started','tool_receipt_reused','tool_exited','tool_cancelled','prompt_result','auto_retry_start','auto_compaction_start','model_policy_rejected','ownership_unresolved','process_exit','unknown'];
for(const type of kinds)for(const v of [undefined,null,'value',0,1,false,true,{},[]])inputs.push({op:'frame',event:{type,hostPid:v,ompPid:42,model:v,restored:{resumed:v},toolName:v,args:{path:'a'},arguments:{path:'b'},isError:v,commandId:v,result:{exitCode:v},status:v,error:{message:v},attempt:v,reason:v,code:v,signal:v}});
for(const event of [{type:'thinking_start'},{type:'text_delta',delta:'hello\nworld'},{type:'text_delta',delta:'!'}, {type:'toolcall_end',toolCall:{name:'write',arguments:{path:'a',content:'one\ntwo'}}},{type:'thinking_start'}])inputs.push({op:'frame',event:{type:'message_update',assistantMessageEvent:event}});
for(const chunk of ['','a','a\n','a\nb','a\n\n','a\r\nb\r\n','\r\r\nx'])inputs.push({op:'output',chunk});
for(const value of [null,'text',3,[],['a','b'],{}, {'9':true,'2':true,z:true,a:true,b:true},{path:'a'.repeat(150)},{content:'one\ntwo\n'},{specialists:[{name:'a'},null,{}, {name:0}]},{path:'a',checkId:12,commandId:2,to:'b',name:'c',messageId:'d',disposition:'queued'}])inputs.push({op:'summary',value});
let now=0, output='';const stream=new ObservationStream(()=>now);const pane=new PaneFormatter(s=>{output+=s},()=>new Date(2026,0,1,12,34,56));
const normalized=JSON.parse(JSON.stringify(inputs));
const expected=normalized.map((v:any)=>{switch(v.op){case 'compact':return compactObservation(v.event);case 'observe':now=v.now;return stream.accept(v.event)??null;case 'frame':output='';pane.frame(v.event);return output;case 'output':output='';pane.output(v.chunk);return output;case 'summary':return summarizeArguments(v.value);}});
// Retain raw equivalent numeric spellings at the Rust parser boundary.
const source=JSON.stringify(normalized).replaceAll('"contentIndex":0,','"contentIndex":-0.0,').replaceAll('"contentIndex":1,','"contentIndex":1.0,');
const child=spawnSync('target/debug/examples/observation-oracle',[],{input:source,encoding:'utf8',maxBuffer:16*1024*1024});
assert.equal(child.status,0,child.stderr);const actual=JSON.parse(child.stdout);const oracle=JSON.parse(JSON.stringify(expected));
for(let i=0;i<oracle.length;i++)assert.deepEqual(actual[i],oracle[i],JSON.stringify({index:i,input:normalized[i]}));
console.log(JSON.stringify({status:'passed',comparisons:oracle.length,corpusSha256:createHash('sha256').update(source).digest('hex'),boundary:'Wire JSON observation and pane presentation; native truncation retains complete Unicode scalars. Pane output is never an execution or quality gate.'},null,2));
