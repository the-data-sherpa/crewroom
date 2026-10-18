/** Disposable native wall fixture. Real checked host/tmux/OMP-shaped RPC processes;
 * scripted provider and read-only HTTP/WS facade. No live provider or ticket delivery. */
import {createServer} from 'node:http';
import {WebSocketServer} from 'ws';
import {randomUUID,randomBytes} from 'node:crypto';
import {mkdtempSync,mkdirSync,writeFileSync,realpathSync,existsSync} from 'node:fs';
import {tmpdir,homedir} from 'node:os';
import {join,relative,resolve} from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import {Workbench} from '../../server/workbench';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
import {NativeSupervisor} from './supervisor-adapter';
import {backend,until} from './host-fixture';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-wall-'))),runtime=join(root,'runtime'),database=join(root,'workflow.sqlite');
mkdirSync(runtime,{mode:0o700});
const b=await backend(),store=new Workbench(database);
const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};
const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
const task=store.createTask({title:'Native agent wall fixture',description:'Four isolated scripted runner processes; no live delivery.'});
store.refine(task.id,{revision:1,sourceRevision:1,brief:'Inspect independent native terminal observations',criteria:['Read-only observation preserves all agent processes']});
const plan=store.queueResearch(task.id,{revision:2,requestId:randomUUID()});const run=store.setDesiredState(plan.id,'running');
const seeds=['Coordinator','Engineer','Reviewer','Technical writer'].map((name,index)=>store.admitAssignment(run.id,{name,role,model,coordinator:index===0,input:'Read-only terminal fixture',brief:{criteria:[],dependsOn:[]}}));store.close();
const c=new NativeSupervisor({database,root:runtime,authority:join(root,'data'),backend:b.origin});await c.ready;
for(const [i,seed]of seeds.entries()){
 const dir=join(runtime,seed.attempt.id);mkdirSync(dir,{recursive:true,mode:0o700});writeFileSync(join(dir,'fixture-settings.json'),'{}',{mode:0o600});
 await c.command('launch',{target:seed.attempt.id,revision:run.controlRevision,config:{providerToken:'synthetic-provider-token-private',model,command:'/usr/bin/env',args:['python3','-u',resolve('scripts/rust/host-fixture-rpc.py')],env:{PATH:process.env.PATH,HOME:homedir(),PI_CONFIG_DIR:relative(homedir(),join(dir,'omp-root')),TMPDIR:dir},upstreamProvider:{baseUrl:b.origin+'/v1',api:'openai-responses',apiKey:'synthetic-upstream-secret',models:[{id:'model',contextWindow:1050000,maxTokens:128000}]},recipients:seeds.map(v=>v.assignment.name),coordinator:i===0}});
 const output=`${seed.assignment.name}: 日本語 🦀 café · native read-only output\n`;
 const events=[...Array.from({length:230},(_,line)=>({type:'message_update',assistantMessageEvent:{type:'text_delta',delta:`row ${line+1} · ${output}`}})),{type:'message_end',message:{role:'assistant',content:[{type:'text',text:output+'Emitted 230 numbered terminal lines.'}]}}];
 await c.command('request',{target:seed.attempt.id,input:{op:'input',id:`native-${i}`,message:JSON.stringify({events})}});
}
await c.command('activate',{target:run.id});
await until(async()=> (await c.command('events',{target:run.id})).filter((v:any)=>v.event.type==='prompt_result').length===4);
const token=randomBytes(32).toString('hex');
const server=createServer(async(req,res)=>{try{
 if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401);res.end('{}');return;}
 const u=new URL(req.url!,'http://127.0.0.1');const parts=u.pathname.split('/').filter(Boolean);let v:any;
 if(req.method!=='GET')throw new Error('Fixture is read-only');
 if(parts.join('/')==='api/workspace')v=await snapshot();
 else if(parts[0]==='api'&&parts[1]==='runs'&&parts[2]===run.id){
  if(parts[3]==='runtime'){v=await c.command('runtime',{target:run.id});v.observedAttemptIds=seeds.map(s=>s.attempt.id);v.degradedAttemptIds=[];}
  else if(parts[3]==='events')v=await c.command('eventpage',{target:run.id,after:Number(u.searchParams.get('after')??0)});
  else if(parts[3]==='attempts'&&parts[5]==='terminal')v=await c.command('terminal',{runId:run.id,target:parts[4]});
  else throw new Error('Unknown fixture route');
 }else throw new Error('Unknown fixture run');
 res.setHeader('Content-Type','application/json');res.end(JSON.stringify(v));
 }catch(e){res.writeHead(409,{'Content-Type':'application/json'});res.end(JSON.stringify({error:String(e)}));}});
const ws=new WebSocketServer({server});
async function snapshot(){const v=await c.command('snapshot');v.capabilities={execution:true};v.connections={runner:true};return v;}
ws.on('connection',(socket,req)=>{if(req.headers.authorization!==`Bearer ${token}`){socket.close();return;}socket.on('message',async()=>socket.send(JSON.stringify({kind:'state',topic:'workspace',data:await snapshot()})));});
await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));const address=server.address() as {port:number};
writeFileSync(join(root,'native-connection.json'),JSON.stringify({protocol:1,origin:`http://127.0.0.1:${address.port}`,token}),{mode:0o600});
const identities=await c.command('runtime',{target:run.id});
writeFileSync(join(root,'fixture.json'),JSON.stringify({root,runId:run.id,taskId:task.id,attempts:identities.attempts,supervisorPid:c.proc.pid},null,2),{mode:0o600});
console.log(JSON.stringify({root,runId:run.id,descriptor:join(root,'native-connection.json'),attempts:seeds.map(s=>s.attempt.id)}));
let closing=false;
async function close(){if(closing)return;closing=true;await c.close();for(const s of ws.clients)s.terminate();ws.close();server.closeAllConnections();server.close();b.server.closeAllConnections();b.server.close();writeFileSync(join(root,'closed.json'),JSON.stringify({exitCode:c.proc.exitCode,stderr:c.stderr}));process.exit(c.proc.exitCode===0?0:1);}
process.on('SIGTERM',()=>void close());process.on('SIGINT',()=>void close());

// Opt-in sustained load starts only after the operator creates this disposable
// fixture's start-load file. The UI can be attached before output begins.
if(process.argv.includes('--load')) void (async()=>{
 while(!closing&&!existsSync(join(root,'start-load')))await sleep(250);
 if(closing)return;
 const started=Date.now(),durationMs=120000;let rounds=0,lines=0;const failures:string[]=[];
 while(!closing&&Date.now()-started<durationMs){
  rounds++;
  for(const [i,seed]of seeds.entries()){
   const output=Array.from({length:50},(_,line)=>({type:'message_update',assistantMessageEvent:{type:'text_delta',delta:`LOAD ${rounds}.${line+1} ${seed.assignment.name} 日本語 🦀 café ${'x'.repeat(96)}\n`}}));
   const events=[...output,{type:'message_end',message:{role:'assistant',content:[{type:'text',text:`${seed.assignment.name}: saved load round ${rounds}`}]}}];
   try{await c.command('request',{target:seed.attempt.id,input:{op:'input',id:`load-${rounds}-${i}`,message:JSON.stringify({events})}});lines+=50;}catch(error){failures.push(String(error));}
  }
  writeFileSync(join(root,'load-progress.json'),JSON.stringify({rounds,lines,elapsedMs:Date.now()-started,failures}));
  await sleep(500);
 }
 if(!closing)writeFileSync(join(root,'load-complete.json'),JSON.stringify({rounds,lines,elapsedMs:Date.now()-started,failures,runtime:await c.command('runtime',{target:run.id})},null,2));
})();
