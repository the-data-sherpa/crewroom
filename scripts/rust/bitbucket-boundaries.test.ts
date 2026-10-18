import assert from 'node:assert/strict';
import {test,type TestContext} from 'node:test';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {BitbucketClient,type BitbucketCredential} from '../../server/bitbucket-client';
import {NativeBitbucketClient} from './provider-client-adapter';
const repository={workspace:'fixture',slug:'repository',uuid:'{00000000-0000-4000-8000-000000000011}'};
const head='a'.repeat(40),base='b'.repeat(40),date='2026-09-28T00:00:00Z';
const pr=(id=7)=>({id,title:'Fixture',description:'Fixture',state:'OPEN',updated_on:date,source:{branch:{name:'fix/test'},commit:{hash:head},repository:{uuid:repository.uuid}},destination:{branch:{name:'main'},commit:{hash:base},repository:{uuid:repository.uuid}},links:{html:{href:`https://bitbucket.org/fixture/repository/pull-requests/${id}`}}});
async function http(t:TestContext,handler:(req:IncomingMessage,res:ServerResponse)=>void){const server=createServer(handler);await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));t.after(async()=>{server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));});return `http://127.0.0.1:${(server.address() as {port:number}).port}`;}
function clients(t:TestContext,apiUrl:string,credentials=async():Promise<BitbucketCredential>=>'synthetic'){const native=new NativeBitbucketClient({repository,apiUrl,credentials});t.after(()=>native.close());return [new BitbucketClient({repository,apiUrl,credentials}),native] as const;}
async function result(f:()=>Promise<unknown>){try{return {ok:true,value:await f()};}catch(error:any){return {ok:false,status:error.status,sent:error.requestSent};}}

test('native Bitbucket basic authorization uses UTF-8 and credential refusal precedes dispatch',async t=>{
 let wire:string|undefined;let calls=0;const api=await http(t,(req,res)=>{calls++;wire=req.headers.authorization;res.end(JSON.stringify({uuid:repository.uuid,full_name:'fixture/repository'}));});
 for(const credential of [{scheme:'basic',username:'é@example.test',token:'synthetic:token'}, {scheme:'basic',username:'bad:name',token:'synthetic'}, {scheme:'basic',username:'',token:'synthetic'}, {scheme:'basic',username:'fixture',token:'bad\nvalue'},'synthetic'] as BitbucketCredential[]){const pair=clients(t,api,async()=>credential);const outcomes=[];for(const client of pair){const before=calls;wire=undefined;outcomes.push({result:await result(()=>client.verifyRepository()),wire,calls:calls-before});}assert.deepEqual(outcomes[1],outcomes[0]);}
});

test('native Bitbucket PR identity and abbreviation failures remain distinct from accepted mutation outcomes',async t=>{
 let mode='normal';let calls=0;const api=await http(t,(req,res)=>{calls++;if(req.url?.includes('/commit/')){if(mode==='lookup404'){res.statusCode=404;res.end('{}');return;}res.end(JSON.stringify({hash:(mode==='wrongExpansion'?'c': 'a').repeat(40)}));return;}
  const value:any=pr(mode==='wrongId'?8:7);if(mode==='uuid')value.source.repository.uuid='{00000000-0000-0000-0000-000000000000}';if(mode==='href')value.links.html.href='https://elsewhere.invalid/7';if(['abbreviated','lookup404','wrongExpansion'].includes(mode))value.source.commit.hash=head.slice(0,12);if(mode==='schema')value.updated_on='not a date';res.end(JSON.stringify(value));
 });const pair=clients(t,api);
 for(mode of ['normal','wrongId','uuid','href','abbreviated','lookup404','wrongExpansion','schema']){
  for(const operation of ['read','create','update']){const outcomes=[];for(const client of pair){calls=0;outcomes.push({result:await result(()=>operation==='read'?client.pullRequest(7):operation==='create'?client.createPullRequest({title:'Fixture',description:'Fixture',branch:'fix/test',target:'main',head}):client.updatePullRequest(7,{title:'Fixture',description:'Fixture'})),calls});}assert.deepEqual(outcomes[1],outcomes[0],mode+' '+operation);}
 }
});

test('native Bitbucket pagination preserves exact endpoint bounds and valid partial comments',async t=>{
 let mode='normal',calls=0;let api='';const comment=(id:number)=>({id,created_on:date,updated_on:date,content:{raw:'Comment'},extra:'strip'});
 api=await http(t,(req,res)=>{calls++;const url=new URL(req.url!,'http://fixture');const page=Number(url.searchParams.get('page')??0);let data:any;
  if(mode==='itemLimit')data={values:Array.from({length:10001},(_,i)=>comment(i+1))};
  else {const row:any=comment(page?2:1);if(page&&mode==='duplicate')row.id=1;if(page&&mode==='wrongPr')row.pullrequest={id:8};if(page&&mode==='invalid')row.inline={path:'file',to:0};if(mode==='deleted'){row.deleted=true;row.resolution=null;delete row.content;}
   data={values:[row]};if(!page||mode==='cycle'||mode==='pages')data.next=mode==='foreign'?`http://127.0.0.2:1${url.pathname}?page=1`:mode==='endpoint'?`${api}/2.0/repositories/other/repository/pullrequests/7/comments?page=1`:mode==='fragment'?`${api}${url.pathname}?page=1#fragment`:`${api}${url.pathname}?page=${mode==='pages'?page+1:1}${mode==='emptyFragment'?'#':''}`;
  }res.end(JSON.stringify(data));
 });const pair=clients(t,api);
 for(mode of ['normal','duplicate','wrongPr','invalid','deleted','foreign','endpoint','fragment','emptyFragment','cycle','pages','itemLimit']){const outcomes=[];for(const client of pair){calls=0;const value=await client.comments(7);outcomes.push({state:value.state,comments:value.comments,calls});}assert.deepEqual(outcomes[1],outcomes[0],mode);}
});

test('native hosted observation selects the latest matching pipeline and rejects stale or ambiguous evidence',async t=>{
 let mode='normal',reads=0;const pipeline=(build:number,run:number,extra:Record<string,unknown>={})=>({uuid:`pipeline-${build}-${run}`,build_number:build,run_number:run,created_on:date,state:{name:'COMPLETED',result:{name:'SUCCESSFUL'}},target:{type:'pipeline_pullrequest_target',commit:{hash:head},destination_commit:{hash:base},pullrequest:{id:'7'},source:'fix/test',destination:'main',selector:{type:'pull-requests',pattern:'**'}},...extra});
 const api=await http(t,(req,res)=>{const url=new URL(req.url!,'http://fixture');let value:any;
  if(url.pathname.endsWith('/pullrequests/7')){reads++;value=pr();if(mode==='changed'&&reads===2)value.title='Changed';}
  else if(url.pathname.endsWith('/statuses')){const status={key:'pr',name:'PR',state:'SUCCESSFUL',url:'https://bitbucket.org/fixture/repository/pipelines/results/2',updated_on:date};value={values:mode==='statusDuplicate'?[status,status]:[status]};}
  else if(url.pathname.endsWith('/steps/')){assert.ok(url.pathname.includes('pipeline-2-2'));const step={uuid:'step',name:'Test',state:{name:'COMPLETED',result:{name:'SUCCESSFUL'}}};value={values:mode==='stepDuplicate'?[step,step]:[step]};}
  else {const chosen=pipeline(2,2);if(mode==='coercedArray')chosen.target.pullrequest.id=[7] as any;if(mode==='invalidCoercion')chosen.target.pullrequest.id='no';const wrong=pipeline(99,1);wrong.target.source='other';value={values:[pipeline(1,1),pipeline(2,1),chosen,wrong]};if(mode==='invalidUnrelated')value.values[3].state=null;}
  res.end(JSON.stringify(value));
 });const pair=clients(t,api);
 for(mode of ['normal','coercedArray','invalidCoercion','statusDuplicate','stepDuplicate','changed','invalidUnrelated']){const outcomes=[];for(const client of pair){reads=0;const value=await result(()=>client.observe(7));if(value.ok)delete (value.value as any).observedAt;outcomes.push(value);}assert.deepEqual(outcomes[1],outcomes[0],mode);}
});
