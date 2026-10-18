import {type IncomingMessage,type ServerResponse} from 'node:http';
import {type TestContext} from 'node:test';
import {body,publicationFixture,providerRepository,validatedCandidate} from './publication-workflow-fixture';
export const summary='Keep the modal actions accessible by correcting the source layout and verifying the exact candidate.';
export async function setup(t:TestContext){
 let f!:Awaited<ReturnType<typeof publicationFixture>>;
 const state={prs:[] as any[],posts:0,puts:0,gets:0,requests:[] as string[],comments:[] as any[],status:'SUCCESSFUL',pipeline:'SUCCESSFUL',step:'SUCCESSFUL',build:1,run:1,date:'2026-10-03T00:00:00Z',outage:false,hidden:false,createStatus:0,createReply:'normal',draft:false,extraStep:false,
  intercept:undefined as undefined|((req:IncomingMessage,res:ServerResponse)=>boolean|Promise<boolean>)};
 const reference=(branch:string)=>f.gitAt(f.remote,'rev-parse',`refs/heads/${branch}`);
 const current=(saved:any)=>({...saved,draft:state.draft,updated_on:state.date,source:{...saved.source,commit:{hash:reference(saved.source.branch.name)}},destination:{...saved.destination,commit:{hash:reference(saved.destination.branch.name)}}});
 f=await publicationFixture(t,async(req,res)=>{
  state.requests.push(`${req.method} ${req.url}`);if(state.intercept && await state.intercept(req,res))return;
  const url=new URL(req.url!,'http://fixture'),path=url.pathname;
  const send=(v:any)=>res.end(JSON.stringify(v));const rows=(v:any[])=>send({values:v});
  if(path.endsWith('/repository'))return void send({uuid:providerRepository.uuid,full_name:'fixture/repository'});
  if(path.endsWith('/comments'))return void rows(state.comments);
  if(req.method==='POST'){
   state.posts++;const input=await body(req);
   if(state.createStatus){res.statusCode=state.createStatus;return void send({});}
   const pr={id:state.prs.length+1,title:input.title,description:input.description,state:'OPEN',updated_on:state.date,source:{branch:input.source.branch,commit:input.source.commit,repository:{uuid:providerRepository.uuid}},destination:{branch:input.destination.branch,commit:{hash:reference(input.destination.branch.name)},repository:{uuid:providerRepository.uuid}},links:{html:{href:`https://bitbucket.org/fixture/repository/pull-requests/${state.prs.length+1}`}}};
   state.prs.push(pr);
   if(state.createReply==='lost')return void res.destroy();
   if(state.createReply==='invalid')return void send({accepted:true});
   return void send(current(pr));
  }
  if(req.method==='PUT'){
   state.puts++;const input=await body(req),pr=state.prs.find(p=>p.id===Number(path.split('/').at(-1)));Object.assign(pr,input);return void send(current(pr));
  }
  if(path.endsWith('/pullrequests'))return void rows(state.hidden?[]:state.prs.map(current));
  const prMatch=path.match(/\/pullrequests\/(\d+)$/);if(prMatch){state.gets++;return void send(current(state.prs.find(p=>p.id===Number(prMatch[1]))));}
  if(state.outage){res.statusCode=503;return void send({});}
  if(path.endsWith('/statuses'))return void rows([{key:'build',state:state.status,name:'Build',url:'https://example.invalid/build',updated_on:state.date}]);
  if(path.endsWith('/steps/'))return void rows([{uuid:'step-1',name:'Tests',state:{name:'COMPLETED',result:{name:state.step}}},...(state.extraStep?[{uuid:'step-2',name:'Unexpected',state:{name:'COMPLETED',result:{name:'SUCCESSFUL'}}}]:[])]);
  if(path.endsWith('/pipelines/')){
   const pr=current(state.prs.at(-1));return void rows([{uuid:'pipeline-1',build_number:state.build,run_number:state.run,created_on:state.date,state:{name:'COMPLETED',result:{name:state.pipeline}},target:{type:'pipeline_pullrequest_target',commit:pr.source.commit,destination_commit:pr.destination.commit,source:pr.source.branch.name,destination:pr.destination.branch.name,pullrequest:{id:pr.id},selector:{type:'pull-requests',pattern:'**'}}}]);
  }
  res.statusCode=404;send({});
 });
 const proof=await validatedCandidate(f);
 return {f,state,proof,reference,current,prepare:(text=summary)=>f.prepare({candidateId:proof.candidate.id,summary:text},proof.agents.coordinator.id),async publish(){await f.prepare({candidateId:proof.candidate.id,summary},proof.agents.coordinator.id);await f.flush();return f.record();}};
}
