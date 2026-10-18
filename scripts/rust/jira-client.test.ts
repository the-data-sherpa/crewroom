import assert from 'node:assert/strict';
import {test,type TestContext} from 'node:test';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {JiraClient,jiraSourceSchema} from '../../server/jira-client';
import {NativeJiraClient} from './provider-client-adapter';
const source={siteUrl:'https://fixture.atlassian.net',cloudId:'00000000-0000-4000-8000-000000000022',filterId:'12345',assigneeAccountId:'operator-1',name:'Assigned work'};
const issue=(id:string)=>({id,key:'SIM-1',extra:true,fields:{summary:'  Fixture 😀  ',description:{type:'doc',content:[{type:'paragraph',content:[{text:'Retain this'}]}]},status:{id:'1',name:' Open ',extra:true},priority:null,project:{name:'Fixture',extra:true},updated:'2026-09-28T00:00:00.000+0000',extra:true}});
async function http(t:TestContext,handler:(req:IncomingMessage,res:ServerResponse)=>void){const server=createServer(handler);await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));t.after(async()=>{server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));});return `http://127.0.0.1:${(server.address() as {port:number}).port}`;}
function clients(t:TestContext,apiUrl:string){const native=new NativeJiraClient({source,apiUrl,credentials:async()=> 'synthetic'});t.after(()=>native.close());return [new JiraClient({source,apiUrl,credentials:async()=> 'synthetic'}),native] as const;}
async function result(f:()=>Promise<unknown>){try{return {ok:true,value:await f()};}catch(error:any){return {ok:false,status:error.status,sent:error.requestSent};}}

test('native Jira source normalization matches strict metadata, Unicode, UUID and empty URL delimiters',async t=>{
 const cases:any[]=[source,...['https://fixture.atlassian.net/','https://fixture.atlassian.net?','https://fixture.atlassian.net#','https://fixture.atlassian.net/?#','http://fixture.atlassian.net','https://x@y.test','https://fixture.atlassian.net/path','https://fixture.atlassian.net/?x','https://fixture.atlassian.net/#x'].map(siteUrl=>({...source,siteUrl})),
  ...['00000000-0000-0000-0000-000000000000','ffffffff-ffff-ffff-ffff-ffffffffffff','250BC9C4-1198-4E6C-9166-2249321DD7E7','{00000000-0000-4000-8000-000000000022}','250bc9c411984e6c91662249321dd7e7'].map(cloudId=>({...source,cloudId})),
  ...['\ufeff  Name \u00a0','\u0085','😀'.repeat(120),'😀'.repeat(121),'   '].map(name=>({...source,name})),{...source,filterId:'01'},{...source,filterId:'1.0'},{...source,assigneeAccountId:'a:b_C-1'},{...source,assigneeAccountId:'a b'},{...source,reviewStatusId:'3',reviewTransitionId:'4'},{...source,reviewStatusId:null},{...source,extra:true}];
 for(const input of cases){const parsed=jiraSourceSchema.safeParse(input);const native=new NativeJiraClient({source:input,apiUrl:'http://127.0.0.1:1',credentials:async()=>{throw new Error('No transport expected');}});try{if(parsed.success){await native.ready;assert.deepEqual(await native.source(),parsed.data);}else{await assert.rejects(native.ready);}}finally{await native.close();}}
});

test('native Jira fixture endpoints reject remote origins and normalize harmless empty delimiters',async t=>{
 for(const apiUrl of ['http://127.0.0.1:1','http://127.0.0.1:1/?#','https://127.0.0.1:1','http://localhost:1','http://127.0.0.2:1','http://127.0.0.1:1/path','http://127.0.0.1:1/?x','http://user@127.0.0.1:1']){
  let accepted=true;try{new JiraClient({source,apiUrl,credentials:async()=> 'unused'});}catch{accepted=false;}
  const native=new NativeJiraClient({source,apiUrl,credentials:async()=>{throw new Error('No transport expected');}});try{if(accepted)await native.ready;else await assert.rejects(native.ready);}finally{await native.close();}
 }
});

test('native Jira wire methods preserve exact IDs, body structure and bounded comment progress',async t=>{
 let mode='issue';const ledger:{method:string,url:string,body:unknown}[]=[];
 const id='100/é ?';const api=await http(t,(req,res)=>{let raw='';req.setEncoding('utf8');req.on('data',v=>raw+=v);req.on('end',()=>{
  ledger.push({method:req.method!,url:req.url!,body:raw?JSON.parse(raw):undefined});const url=new URL(req.url!,'http://fixture');let value:unknown;
  if(mode==='issue'||mode==='wrongIssue')value=issue(mode==='issue'?id:'different');
  else if(mode==='comments')value=url.searchParams.get('startAt')==='0'?{startAt:0,total:2,comments:[{id:'c1',body:{text:'One'},extra:true}]}:{startAt:1,total:2,comments:[{id:'c2',body:null}]};
  else if(mode==='wrongStart')value={startAt:1,total:1,comments:[{id:'c1',body:{}}]};
  else if(mode==='noProgress')value={startAt:0,total:1,comments:[]};
  else if(mode==='missingBody')value={startAt:0,total:1,comments:[{id:'c1'}]};
  else if(mode==='transitions')value={transitions:[{id:'3',to:{id:'4',extra:true},fields:{summary:{required:true,extra:true},optional:{}},extra:true}]};
  else if(mode==='add')value={id:'c3',extra:true};
  else{res.statusCode=204;res.end();return;}res.end(JSON.stringify(value));
 });});const pair=clients(t,api);
 for(mode of ['issue','wrongIssue','comments','wrongStart','noProgress','missingBody','transitions','add','transition']){
  const results=[];const requests=[];
  for(const client of pair){ledger.length=0;results.push(await result(()=>mode==='issue'||mode==='wrongIssue'?client.issue(id):['comments','wrongStart','noProgress','missingBody'].includes(mode)?client.comments(id):mode==='transitions'?client.transitions(id):mode==='add'?client.addComment(id,{type:'doc',content:[]}):client.transition(id,'6')));requests.push(structuredClone(ledger));}
  assert.deepEqual(results[1],results[0],mode);assert.deepEqual(requests[1],requests[0],mode);assert.ok(requests[0][0].url.includes('100%2F%C3%A9%20%3F'));
 }
});

test('native Jira search matches query ordering, all-page validation, duplicate and token limits',async t=>{
 let mode='normal';let requests=0;const api=await http(t,(req,res)=>{
  requests++;const url=new URL(req.url!,'http://fixture');assert.equal(url.pathname,'/rest/api/3/search/jql');assert.equal(url.searchParams.get('jql'),'filter = 12345 AND assignee = "operator-1" ORDER BY Rank ASC');assert.equal(url.searchParams.get('fields'),'summary,description,status,priority,project,updated');assert.equal(url.searchParams.get('maxResults'),'100');
  const page=Number(url.searchParams.get('nextPageToken')??0);let data:unknown;
  if(mode==='normal')data={issues:[issue(page?'2':'1')],isLast:!!page,...(page?{}:{nextPageToken:'1'})};
  else if(mode==='duplicate')data={issues:[issue('1')],isLast:!!page,...(page?{}:{nextPageToken:'1'})};
  else if(mode==='cycle')data={issues:[],isLast:false,nextPageToken:'1'};
  else if(mode==='missing')data={issues:[],isLast:false};
  else if(mode==='pageLimit')data={issues:[],isLast:false,nextPageToken:String(page+1)};
  else if(mode==='itemLimit')data={issues:Array.from({length:1001},(_,n)=>issue(String(n))),isLast:true};
  else data={issues:page?[{...issue('2'),fields:{summary:''}}]:[issue('1')],isLast:!!page,...(page?{}:{nextPageToken:'1'})};res.end(JSON.stringify(data));
 });const pair=clients(t,api);
 for(mode of ['normal','duplicate','cycle','missing','pageLimit','itemLimit','invalidLater']){const results=[];for(const client of pair){requests=0;results.push({result:await result(()=>client.search()),requests});}assert.deepEqual(results[1],results[0],mode);}
});
