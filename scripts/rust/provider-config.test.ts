import assert from 'node:assert/strict';
import {test,type TestContext} from 'node:test';
import {mkdtempSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createServer} from 'node:http';
import {gzipSync} from 'node:zlib';
import {setTimeout as sleep} from 'node:timers/promises';
import {NativeProviderConfig,type Command} from './provider-config-adapter';
import {loadPublicationConnections} from '../../server/publication-connection';
const now=Date.parse('2026-10-03T10:00:00Z');
const source={siteUrl:'https://fixture.atlassian.net',cloudId:'00000000-0000-4000-8000-000000000022',filterId:'12345',assigneeAccountId:'operator-1',name:'Assigned work'};
const repository={workspace:'fixture',slug:'repository',uuid:'{00000000-0000-4000-8000-000000000011}'};
const entry={repositoryPath:'/disposable/repository',repository,policy:{statusKeys:['pr'],pipelineSteps:['Test']}};
function encoded(token:unknown='synthetic-acli',expiry:unknown=new Date(now+3600000).toISOString(),format='go-keyring-base64') {const bytes=gzipSync(JSON.stringify({access_token:token,expiry}));return `${format}:${bytes.toString(format==='go-keyring-encoded'?'hex':'base64')}`;}
function account(method='basic',extra:Record<string,unknown>={}){return {active_context:'conductor',contexts:[{name:'conductor',host:'api.bitbucket.org'}],hosts:[{key:'api.bitbucket.org',kind:'cloud',base_url:'https://api.bitbucket.org/2.0',auth_method:method,token_source:'keyring',username:'fixture@example.test',...extra}]};}
async function fixture(t:TestContext,provider:'jira'|'bitbucket',metadata:unknown,command:(c:Command)=>Promise<string>,environment?:string){
 const root=mkdtempSync(join(tmpdir(),'cr-native-provider-config-'));const path=join(root,'connection.json');if(metadata!==undefined)writeFileSync(path,JSON.stringify(metadata));
 const headers:string[]=[];const server=createServer((req,res)=>{headers.push(req.headers.authorization??'');res.end(JSON.stringify(provider==='jira'?{ok:true}:{uuid:repository.uuid,full_name:'fixture/repository'}));});await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));
 const client=new NativeProviderConfig({provider,path,now,environment,apiUrl:`http://127.0.0.1:${(server.address() as {port:number}).port}`},command);
 t.after(async()=>{await client.close();server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));rmSync(root,{recursive:true,force:true});});return {client,headers,path};
}
async function waitFor(check:()=>boolean){const until=Date.now()+5000;while(!check()){if(Date.now()>until)throw new Error('Fixture latch timed out');await sleep(5);}}

test('native private metadata is strict, token-free and matches retained publication defaults',async t=>{
 for(const metadata of [undefined,[],[entry],[{...entry,policy:{...entry.policy,optionalSteps:['Optional']}}],Array.from({length:20},()=>entry),Array.from({length:21},()=>entry),[{...entry,repositoryPath:'relative'}],[{...entry,token:'synthetic-forbidden'}],[{...entry,policy:{...entry.policy,statusKeys:[]}}]]){
  const {client,path}=await fixture(t,'bitbucket',metadata,async()=>{throw new Error('No credential reads expected');});let expected:unknown;let valid=true;try{expected=loadPublicationConnections(path,async()=>{throw new Error('No credential reads expected');}).map(({credentials,...m})=>m);}catch{valid=false;}
  if(valid){const actual=await client.ready;assert.deepEqual(actual,expected);assert.equal(JSON.stringify(actual).includes('synthetic'),false);}else await assert.rejects(client.ready);
 }
 const missing=await fixture(t,'jira',undefined,async()=>{throw new Error('No command expected');});assert.equal(await missing.client.ready,null);
 for(const metadata of [{...source,accountId:'a:b_C-1'},{...source,accountId:'bad account'},{...source,accountId:'operator',token:'synthetic-forbidden'}]){const {client}=await fixture(t,'jira',metadata,async()=>{throw new Error('No command expected');});if(metadata.accountId==='a:b_C-1')assert.deepEqual(await client.ready,source);else await assert.rejects(client.ready);}
});

test('native Jira keyring formats decode privately, including padding, concatenation and accepted expiry forms',async t=>{
 const json=JSON.stringify({access_token:'synthetic-acli',expiry:new Date(now+3600000).toISOString()});const concatenated=Buffer.concat([gzipSync(json.slice(0,30)),gzipSync(json.slice(30))]);
 const records=[encoded(),encoded('synthetic-acli',new Date(now+3600000).toISOString(),'go-keyring-encoded'),encoded().replace(/=+$/,'')+' !!',`go-keyring-base64:${concatenated.toString('base64url')}`,encoded('synthetic-acli',2200),encoded('synthetic-acli',['2200'])];
 for(const record of records){const commands:Command[]=[];const {client,headers}=await fixture(t,'jira',{...source,accountId:'operator'},async c=>{commands.push(c);return record+'\n';});assert.deepEqual(await client.ready,source);await client.call();assert.deepEqual(headers,['Bearer synthetic-acli']);assert.deepEqual(commands,[{file:'/usr/bin/security',args:['find-generic-password','-s','acli','-a',`oauth:${source.cloudId}:operator`,'-w'],timeout:10000,maxBuffer:1024*1024}]);}
});

test('native Jira expired and empty tokens share one renewal and reread before delivery',async t=>{
 for(const token of ['expired','']){let reads=0,renewals=0;let release!:()=>void;const held=new Promise<void>(r=>release=r);let fresh=false;
  const {client,headers}=await fixture(t,'jira',{...source,accountId:'operator'},async c=>{if(c.file.endsWith('/security')){reads++;return encoded(fresh?'synthetic-new':token,new Date(now+(fresh?3600000:token?30000:3600000)).toISOString());}renewals++;assert.deepEqual(c,{file:'/opt/homebrew/bin/acli',args:['jira','workitem','search','--filter',source.filterId,'--limit','1','--json'],timeout:30000,maxBuffer:2*1024*1024});await held;fresh=true;return '{}';});
  const jobs=Array.from({length:8},()=>client.call());await waitFor(()=>reads===8&&renewals===1);release();await Promise.all(jobs);assert.equal(renewals,1);assert.equal(reads,9);assert.equal(headers.length,8);assert.ok(headers.every(v=>v==='Bearer synthetic-new'));
 }
});

test('native Jira malformed, oversized and failed credential reads never leak output or dispatch',async t=>{
 const oversized=`go-keyring-base64:${gzipSync(JSON.stringify({access_token:'synthetic-secret',expiry:'2200',padding:'x'.repeat(1024*1024)})).toString('base64')}`;
 for(const record of ['unsupported:synthetic-secret','go-keyring-base64:broken',oversized,encoded('synthetic-secret',null),'command-failure']){
  let calls=0;const {client,headers}=await fixture(t,'jira',{...source,accountId:'operator'},async()=>{calls++;if(record==='command-failure')throw new Error('synthetic-secret');return record;});
  await assert.rejects(client.call(),(e:any)=>e.requestSent===false&&!e.message.includes('synthetic-secret')&&/credentials are unavailable/.test(e.message));assert.equal(headers.length,0);assert.equal(calls,record===encoded('synthetic-secret',null)?3:1);
 }
});

test('native Bitbucket credentials share refresh, cache across repositories and expire at the margin',async t=>{
 const commands:Command[]=[];let generation=1;
 const {client,headers}=await fixture(t,'bitbucket',[entry,{...entry,repositoryPath:'/disposable/second'}],async c=>{commands.push(c);if(c.args.includes('status'))return JSON.stringify(account('basic',{username:'é@example.test'}));if(c.file.endsWith('/security'))return `synthetic-${generation}\r\n`;return '{}';});
 await Promise.all(Array.from({length:12},(_,index)=>client.call('probe',{index:index%2})));assert.equal(commands.length,3);assert.equal(headers.length,12);assert.ok(headers.every(v=>v==='Basic '+Buffer.from('é@example.test:synthetic-1').toString('base64')));
 assert.deepEqual(commands.map(c=>[c.file,c.args,c.timeout,c.maxBuffer]),[
  ['/opt/homebrew/bin/bkt',['--context','conductor','auth','status','--json'],30000,1024*1024],['/opt/homebrew/bin/bkt',['--context','conductor','api','/user','--json'],30000,1024*1024],['/usr/bin/security',['find-generic-password','-s','bkt','-a','host/api.bitbucket.org/token','-w'],10000,1024*1024]]);
 await client.call('clock',{value:now+269999});await client.call();assert.equal(commands.length,3);
 generation=2;await client.call('clock',{value:now+270000});await Promise.all(Array.from({length:8},()=>client.call()));assert.equal(commands.length,6);assert.equal(headers.at(-1),'Basic '+Buffer.from('é@example.test:synthetic-2').toString('base64'));
 await client.call('environment',{value:'synthetic-environment'});await client.call();assert.equal(headers.at(-1),'Bearer synthetic-environment');assert.equal(commands.length,6);await client.call('environment',{value:''});await client.call();assert.ok(headers.at(-1)?.startsWith('Basic '));
});

test('native Bitbucket OAuth expiry and unavailable state preserve explicit environment override',async t=>{
 for(const mode of ['valid','expired','wrongHost','wrongMethod','wrongContext','badUsername','badToken','commandFailure']){
  let commands=0;const {client,headers}=await fixture(t,'bitbucket',[entry],async c=>{commands++;if(mode==='commandFailure')throw new Error('synthetic-secret');if(c.args.includes('status'))return JSON.stringify({...account(['badUsername','badToken'].includes(mode)?'basic':'oauth',mode==='wrongHost'?{base_url:'https://elsewhere.invalid/2.0'}:mode==='wrongMethod'?{auth_method:'token'}:mode==='badUsername'?{username:'bad:name'}:{}),...(mode==='wrongContext'?{active_context:'elsewhere'}:{})});if(c.file.endsWith('/security'))return mode==='badToken'?'synthetic-secret\n\n':mode==='badUsername'?'synthetic-secret\n':JSON.stringify({access_token:'synthetic-oauth',expires_at:new Date(now+(mode==='expired'?30000:90000)).toISOString()});return '{}';});
  if(mode==='valid'){await client.call();assert.deepEqual(headers,['Bearer synthetic-oauth']);assert.equal(commands,3);await client.call('clock',{value:now+60000});await assert.rejects(client.call());assert.equal(commands,6);}else{await assert.rejects(client.call(),(e:any)=>e.requestSent===false&&!e.message.includes('synthetic-secret'));assert.equal(headers.length,0);}
  const count=commands;await assert.rejects(client.call());assert.equal(commands,count,'Keychain failure stays unavailable until reconfiguration');await client.call('environment',{value:'synthetic-override'});await client.call();assert.equal(headers.at(-1),'Bearer synthetic-override');assert.equal(commands,count);await client.call('environment',{value:null});await assert.rejects(client.call());assert.equal(commands,count);
 }
});
