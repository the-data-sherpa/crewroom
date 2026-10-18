import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, existsSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { UsageReader } from '../../server/runtime/provider-usage';
import { hasBoundedTextInput, ReasoningBudget } from '../../server/runtime/provider-input';
const cases:any[]=[];
const apis=['openai-responses','anthropic-messages','google-generative-ai','unsupported'];
const blocks=[null,true,0,'text',{},[],{type:'input_text',text:'prose'},{type:'output_text',text:'result'},{type:'refusal',refusal:'no'},
 {type:'input_image',image_url:'https://example.invalid'},{type:'input_file',file_id:'file'}, {type:'input_audio',input_audio:{}},
 {type:'text',text:'{"type":"input_image"}'},{type:'thinking',thinking:'private'},{type:'redacted_thinking',data:'opaque'},
 {type:'tool_use',name:'tool',input:{image_url:'ordinary argument'}},{type:'tool_result',content:[{type:'text',text:'result'}]},
 {type:'tool_result',content:[{type:'image',source:{data:'opaque'}}]}, {text:'text'},{text:'thinking',thought:true,thoughtSignature:'opaque'},
 {text:'text',thoughtSignature:'skip_thought_signature_validator'},{functionCall:{name:'tool',args:{fileData:'ordinary argument'}}},
 {functionResponse:{name:'tool',response:{image:'ordinary result'}}},{functionResponse:{name:'tool',response:{},parts:[{text:'part'}]}},
 {functionResponse:{response:{},parts:[{fileData:{fileUri:'remote'}}]}},{inlineData:{data:'AA=='}},{fileData:{fileUri:'gs://x'}}];
for(const api of apis) for(const opaque of [false,true])for(const block of blocks) {
 const body=api==='openai-responses'?{input:[{role:'user',content:[block]}]}:api==='anthropic-messages'?{messages:[{role:'user',content:[block]}]}:{contents:[{parts:[block]}]};cases.push({op:'input',api,body,opaque});
}
for(const input of ['text',[],null,{},[{type:'function_call',arguments:'{"type":"image"}'}],[{type:'function_call_output',output:'result'}],[{type:'reasoning',encrypted_content:'opaque',summary:[]}],[{type:'reasoning',encrypted_content:null,summary:[{type:'summary_text',text:'summary'}]}],[{type:null,content:'message'}]])
 for(const extra of [{},{previous_response_id:'x'},{conversation:'x'},{prompt:'x'},{tools:[]},{tools:[{type:'function'}]},{tools:[{type:'web_search'}]},{tools:null}])for(const opaque of [false,true])cases.push({op:'input',api:apis[0],body:{input,...extra},opaque});
for(const api of apis.slice(0,3))for(const extra of [{},{tools:null},{tools:[]},{system:null},{system:'text'},{system:[{type:'text',text:'text'}]},{systemInstruction:null},{systemInstruction:{parts:[{text:'text'}]}},{cachedContent:'remote'},{tools:[{type:'custom'}]},{tools:[{functionDeclarations:[]}]},{tools:[{googleSearch:{}}]}])cases.push({op:'input',api,body:{input:'hello',messages:[{content:'hello'}],contents:[{parts:[{text:'hello'}]}],...extra},opaque:false});
const events=[{},null,[], {type:'response.created',response:{usage:null}},
 {type:'response.completed',response:{output:[{type:'message',content:[{type:'output_text',text:'résumé🦀'}]}],usage:{input_tokens:10,output_tokens:20,total_tokens:30,input_tokens_details:{cached_tokens:2},output_tokens_details:{reasoning_tokens:3}}}},
 {type:'response.incomplete',response:{usage:{input_tokens:10,output_tokens:20}}}, {type:'response.failed',response:{usage:{input_tokens:0,output_tokens:0}}},
 {object:'response',status:'completed',output:[{type:'reasoning',encrypted_content:'opaque'}],usage:{input_tokens:5,output_tokens:7}},
 {type:'response.refusal.delta',delta:'No'},{type:'response.completed',response:{output:[]}},
 {type:'message_start',message:{usage:{input_tokens:5,output_tokens:0,cache_read_input_tokens:10,cache_creation_input_tokens:3}}},
 {type:'content_block_start',content_block:{type:'tool_use',name:'call'}},{type:'content_block_delta',delta:{text:'answer'}},
 {type:'message_delta',usage:{output_tokens:12}},{type:'message_stop'},
 {type:'message',content:[{type:'redacted_thinking',data:'opaque'}],stop_reason:'end_turn',usage:{input_tokens:4,output_tokens:6}},
 {candidates:[{content:{parts:[{text:'answer',thoughtSignature:'opaque'}]},finishReason:'STOP'}],usageMetadata:{promptTokenCount:4,candidatesTokenCount:5,thoughtsTokenCount:3,cachedContentTokenCount:1,totalTokenCount:12}},
 {candidates:[{content:{parts:[{thought:true,text:'private'}]},finishReason:'STOP'}]},
 {candidates:[{content:{parts:[{functionCall:{name:'tool'}}]},finishReason:'STOP'}]},
];
const sse=(e:any[])=>e.map(v=>'data: '+JSON.stringify(v)+'\r\n\r\n').join('');
const payloads=[...events.map(e=>JSON.stringify(e)),...events.map(e=>sse([e])),sse(events),JSON.stringify(events),sse(events.slice(10,15)), 'data: {invalid}\n\n','data: [DONE]\n\n','data: {}', '\ufeff'+sse(events),'data: '+JSON.stringify({type:'response.output_text.delta',delta:'\u0085'})+'\n\n'+sse([events[9]])];
function addUsage(api:string,stream:boolean,bytes:Buffer,size:number,status=200,interrupted=false) {const chunks=[];for(let i=0;i<bytes.length;i+=size)chunks.push(bytes.subarray(i,i+size).toString('base64'));cases.push({op:'usage',api,stream,chunks,status,interrupted,finishes:2});}
for(const api of apis)for(const stream of [true,false])for(const payload of payloads)for(const size of [1,17,100000])addUsage(api,stream,Buffer.from(payload),size);
for(const status of [0,199,200,204,299,300,401,403,429,500])for(const interrupted of [false,true])addUsage(apis[0],true,Buffer.from(sse([events[4]])),13,status,interrupted);
for(const bytes of [Buffer.from([0xff,0xfe]),Buffer.from([0xef,0xbb]),Buffer.concat([Buffer.from('data: {"type":"response.output_text.delta","delta":"'),Buffer.from([0xf0,0x9f]),Buffer.from('"}\n\n')])])for(const stream of [true,false])addUsage(apis[0],stream,bytes,1);
for(const n of [null,'1',-1,0,1,1.5,9007199254740991,9007199254740992])for(const api of apis.slice(0,3)) {
 const e=api===apis[0]?{type:'response.completed',response:{usage:{input_tokens:n,output_tokens:n}}}:api===apis[1]?{type:'message',stop_reason:'end',usage:{input_tokens:n,output_tokens:n}}:{candidates:[{finishReason:'STOP'}],usageMetadata:{promptTokenCount:n,candidatesTokenCount:n}};
 addUsage(api,false,Buffer.from(JSON.stringify(e)),23);
}
for(const selector of ['fixture/model','fixture/slash/model','provider/🦀']) {
 const operations:any[]=[];
 for(const output of [20,5,0,100])for(const kind of ['openai-reasoning','anthropic-thinking','google-thinking']) {
  operations.push({type:'tokens',kind,value:'opaque'},{type:'remember',blocks:[{kind,value:'opaque'},{kind,value:'different'}],output},{type:'reopen'},{type:'tokens',kind,value:'opaque'});
 }
 cases.push({op:'budget',selector,operations});
}
for(const saved of [null,[],42,'text',{'bad':2},{['a'.repeat(64)]:-1},{['A'.repeat(64)]:2},{['a'.repeat(64)]:1.5},{['a'.repeat(64)]:null},{['a'.repeat(64)]:9007199254740992},{}])cases.push({op:'budget',selector:'fixture/model',saved,operations:[]});
for(const preset of ['json-at-limit','json-over-limit','line-at-limit','line-over-limit'])cases.push({op:'usage',api:apis[0],stream:preset.startsWith('line'),preset,status:200,interrupted:false,finishes:2});
function presetChunks(preset:string):Buffer[] {
 const event=JSON.stringify({type:'response.completed',response:{usage:{input_tokens:1,output_tokens:2}}});
 const body=JSON.stringify({object:'response',status:'completed',usage:{input_tokens:1,output_tokens:2}});
 if(preset.startsWith('json'))return [Buffer.from(' '.repeat(8*1024*1024-body.length+Number(preset==='json-over-limit'))+body)];
 return [Buffer.from(':'.repeat(1024*1024+Number(preset==='line-over-limit'))),Buffer.from('\ndata: '+event+'\n\n')];
}
const normalized=JSON.parse(JSON.stringify(cases));
const expected=normalized.map((v:any)=>{
 if(v.op==='input'){const calls:any[]=[];const value=hasBoundedTextInput(v.api,v.body,(kind,value)=>{calls.push([kind,value]);return v.opaque});return {value,opaque:calls};}
 if(v.op==='usage'){const opaque:any[]=[];const reader=new UsageReader(v.api,v.stream,(kind,value)=>opaque.push([kind,value]));for(const chunk of v.preset?presetChunks(v.preset):v.chunks.map((c:string)=>Buffer.from(c,'base64')))reader.push(chunk);const receipts=Array.from({length:v.finishes},()=>reader.finish(v.status,v.interrupted));return {receipts,outcome:reader.outcome()??null,opaque};}
 const root=mkdtempSync(join(tmpdir(),'crewroom-native-budget-'));try {
  if('saved' in v)writeFileSync(join(root,'reasoning-budget.json'),JSON.stringify(v.saved));
  let budget=new ReasoningBudget(root,v.selector);const receipts:any[]=[];
  for(const o of v.operations){if(o.type==='remember')budget.remember(o.blocks,o.output);else if(o.type==='reopen')budget=new ReasoningBudget(root,v.selector);else receipts.push(budget.tokens(o.kind,o.value)??null);}
  return {receipts,saved:existsSync(join(root,'reasoning-budget.json'))?JSON.parse(readFileSync(join(root,'reasoning-budget.json'),'utf8')):null};
 }catch(error){return {error:(error as Error).message};}finally{rmSync(root,{recursive:true,force:true});}
});
const source=JSON.stringify(normalized);const child=spawnSync('target/debug/examples/provider-oracle',[],{input:source,encoding:'utf8',maxBuffer:32*1024*1024});assert.equal(child.status,0,child.stderr);const actual=JSON.parse(child.stdout);assert.equal(actual.length,expected.length);
for(let i=0;i<expected.length;i++)assert.deepEqual(actual[i],expected[i],JSON.stringify({index:i,case:normalized[i]}));
console.log(JSON.stringify({status:'passed',comparisons:cases.length,counts:Object.fromEntries(['input','usage','budget'].map(op=>[op,cases.filter(v=>v.op===op).length])),corpusSha256:createHash('sha256').update(source).digest('hex'),scope:'Protocol input positions, reported usage/outcomes/opaque callbacks across split bytes, and private durable reasoning budgets. No provider transport or workflow admission.'},null,2));
