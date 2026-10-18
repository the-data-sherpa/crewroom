/** GPUI controls -> production Rust service -> real owned processes/Git/Docker.
 * OMP RPC, Mail and Bitbucket replies are explicit disposable loopback fixtures. */
import assert from 'node:assert/strict';
import {execFileSync, spawn} from 'node:child_process';
import {once} from 'node:events';
import {randomUUID} from 'node:crypto';
import {readFileSync, writeFileSync} from 'node:fs';
import {createServer} from 'node:http';
import {join} from 'node:path';
import test from 'node:test';
import {NativeDaemon} from './daemon-client';
import {nativeUi} from './native-ui-driver';
import {body} from './jira-workflow-fixture';
import {providerRepository} from './publication-workflow-fixture';
import {reviewedCandidate,reworkedCandidate} from './workflow-coding-fixture';
import {until, workflowFixture,acceptFixturePlan} from './workflow-support';

test('GPUI Resume captures PR feedback, executes a successor and displays fresh evidence on the same PR', {timeout:240000}, async t => {
  const executable = process.env.CREWROOM_NATIVE_TEST_BINARY;
  assert.ok(executable, 'Run scripts/test-native-resume.sh to build the native driver');
  let remote = '', posts = 0, puts = 0;
  const prs: any[] = [], comments: any[] = [];
  const reference = (branch: string) => execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${branch}`], {encoding:'utf8'}).trim();
  const current = () => {
    const pr = prs.at(-1);
    return {...pr, source:{...pr.source, commit:{hash:reference(pr.source.branch.name)}},
      destination:{...pr.destination, commit:{hash:reference(pr.destination.branch.name)}}};
  };
  const provider = createServer(async (req, res) => {
    try {
      assert.equal(req.headers.authorization, 'Bearer synthetic-bitbucket-token');
      res.setHeader('Content-Type', 'application/json');
      const path = new URL(req.url!, 'http://fixture').pathname;
      const send = (value: any) => res.end(JSON.stringify(value));
      if (path.endsWith('/repository')) return void send({uuid:providerRepository.uuid, full_name:'fixture/repository'});
      if (path.endsWith('/comments')) return void send({values:comments});
      if (req.method === 'POST' && path.endsWith('/pullrequests')) {
        const value = await body(req); posts++;
        prs.push({id:posts, title:value.title, description:value.description, state:'OPEN', updated_on:'2026-10-04T00:00:00Z',
          source:{branch:value.source.branch, commit:value.source.commit, repository:{uuid:providerRepository.uuid}},
          destination:{branch:value.destination.branch, repository:{uuid:providerRepository.uuid}},
          links:{html:{href:`https://bitbucket.org/fixture/repository/pull-requests/${posts}`}}});
        return void send(current());
      }
      if (req.method === 'PUT' && /\/pullrequests\/\d+$/.test(path)) {
        puts++; Object.assign(prs.at(-1), await body(req)); return void send(current());
      }
      assert.equal(req.method, 'GET');
      if (path.endsWith('/pullrequests')) return void send({values:prs.length ? [current()] : []});
      if (/\/pullrequests\/\d+$/.test(path)) return void send(current());
      if (path.endsWith('/statuses')) return void send({values:[{key:'build', state:'SUCCESSFUL', name:'Build', url:'https://example.invalid/build', updated_on:'2026-10-04T00:00:00Z'}]});
      if (path.endsWith('/pipelines/')) {
        const pr = current();
        return void send({values:[{uuid:'pipeline-1', build_number:1, run_number:1, created_on:'2026-10-04T00:00:00Z',
          state:{name:'COMPLETED', result:{name:'SUCCESSFUL'}}, target:{type:'pipeline_pullrequest_target', commit:pr.source.commit,
            destination_commit:pr.destination.commit, source:pr.source.branch.name, destination:pr.destination.branch.name,
            pullrequest:{id:pr.id}, selector:{type:'pull-requests', pattern:'**'}}}]});
      }
      if (path.endsWith('/steps/')) return void send({values:[{uuid:'step-1', name:'Tests', state:{name:'COMPLETED', result:{name:'SUCCESSFUL'}}}]});
      res.statusCode = 404; send({});
    } catch (error) { res.statusCode = 500; res.end(JSON.stringify({error:String(error)})); }
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  t.after(async () => { provider.closeAllConnections(); await new Promise<void>(resolve => provider.close(() => resolve())); });
  const apiUrl = `http://127.0.0.1:${(provider.address() as any).port}`;
  const f = await workflowFixture(t, 'coding', 2, false, {service:true, configure({root, source, config}) {
    config.runtime.launchPrefix.push('--unique-sessions');
    remote = join(root, 'remote.git');
    execFileSync('git', ['clone', '--bare', source, remote], {stdio:'ignore'});
    config.publicationRemote = {connections:[{repositoryPath:source, repository:providerRepository,
      policy:{statusKeys:['build'], pipelineSteps:['Tests'], optionalSteps:[]}, fixtureGitPath:remote, apiUrl}]};
  }});
  assert.ok(f.c instanceof NativeDaemon);
  const c = f.c;
  const sourceHead = execFileSync('git', ['-C', f.source, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim();
  const ui = async (action: string, extra: Record<string,unknown> = {}) => {
    const plan = join(f.root, `native-${action}.json`), resultPath = join(f.root, `native-${action}-result.json`);
    writeFileSync(plan, JSON.stringify({descriptor:join(f.config.authority, 'native-connection.json'),
      taskId:f.task.id, runId:f.run.id, action, resultPath, ...extra}), {mode:0o600});
    const child = spawn(executable, ['native_external_feedback_resume', '--ignored', '--test-threads=1', '--nocapture'],
      {env:{...process.env, CREWROOM_NATIVE_JOURNEY_PLAN:plan}, stdio:'pipe'});
    let output = ''; child.stdout.on('data', value => output += value); child.stderr.on('data', value => output += value);
    const timer = setTimeout(() => child.kill('SIGKILL'), 45000);
    let code: number | null;
    try { [code] = await once(child, 'exit') as [number|null]; } finally { clearTimeout(timer); }
    assert.equal(code, 0, output);
    const result = JSON.parse(readFileSync(resultPath, 'utf8')); assert.equal(result.error, '');
    t.diagnostic(`Native ${action}: ${output.trim()}`); return result;
  };
  const record = () => c.command('record', {target:f.run.id});
  const publication = () => c.command('publication', {target:f.run.id});
  const paused = () => until(async () => {
    const value = await f.snapshot(); return value.run.status === 'paused' && value.run.pauseReason === 'review' && !value.busy ? value : false;
  }, 45000);
  const first = await reviewedCandidate(f);
  await f.tool(first.coordinator, 'publication/prepare', {candidateId:first.candidate.id, summary:'Initial independently checked native UI feedback fixture.'});
  await c.request(`/api/runs/${f.run.id}/publication/reconcile`, {});
  const previous = await paused(), initial = await publication();
  assert.equal(initial.ready, true); assert.equal(posts, 1); assert.equal(puts, 0);
  comments.push({id:51, created_on:'2026-10-04T00:00:00Z', updated_on:'2026-10-04T00:00:00Z', deleted:false,
    content:{raw:'Cover the retained feedback case.'}, user:{display_name:'Fixture reviewer'}});
  const operator=(action:string,extra:Record<string,unknown>={})=>nativeUi(t,f.root,{descriptor:join(f.config.authority,'native-connection.json'),taskId:f.task.id,runId:f.run.id,action,...extra});
  const queue=async(title:string,dependency?:string)=>{
    const task=await c.request('/api/tasks',{title,description:'',repositoryId:f.repository.id});
    await c.request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:title,criteria:['Checked change']});
    if(dependency)await c.request(`/api/tasks/${task.id}/dependencies`,{requestId:randomUUID(),dependencyRevision:0,dependsOn:[dependency]});
    await acceptFixturePlan(c,task.id);
    await operator('queue',{taskId:task.id,runId:null});
    return (await c.request('/api/workspace')).runs.find((r:any)=>r.taskId===task.id);
  };
  const running=(id:string)=>until(async()=>{const v=await c.request(`/api/runs/${id}/runtime`);return v.run.status==='running'&&!v.busy?v:false;},45000);
  const holder=await queue('Holding coding slot'),waiting=await queue('Next queued plan');
  await running(holder.id);
  await operator('queue-inspect',{slotRunId:holder.id,waitingRunId:waiting.id,holdText:holder.taskSnapshot.key,runExplanation:'Paused for human review'});
  await ui('resume');
  const queuedResume=await f.snapshot(); assert.equal(queuedResume.run.resumeRequest.state,'queued');
  assert.ok(queuedResume.attempts.every((a:any)=>a.ownership==='stopped'));
  await operator('queue-inspect',{slotRunId:holder.id,waitingRunId:waiting.id,holdText:holder.taskSnapshot.key,runExplanation:'Resume is queued'});
  await c.request(`/api/runs/${holder.id}/cancel`,{});
  let runtime = await f.idle();
  assert.equal(runtime.run.status, 'running');
  assert.ok(runtime.attempts.length > previous.attempts.length);
  for (const assignment of runtime.assignments.filter((a:any)=>!a.retiredAt)) assert.ok(!previous.attempts.some((p:any) => p.id === assignment.currentAttemptId));
  const continuation = runtime.run.feedbackContinuations.at(-1);
  assert.equal(continuation.feedback.comments[0].content.raw, comments[0].content.raw);
  assert.equal(continuation.feedback.comments.length, 1);
  const next=await reworkedCandidate(f,async writer=>{
    const source=await f.tool(writer,'coding/read',{path:'source.txt'});assert.equal(source.content,'after\n');
    await f.tool(writer,'coding/edit',{path:'feedback.txt',expectedHash:null,content:'Covered feedback case\n'});
  });
  const {coordinator,reviewer,candidate}=next;
  const nextRecord=await record();
  assert.equal(nextRecord.candidates.length, 2); assert.notEqual(candidate.revision.head, first.candidate.revision.head);
  assert.deepEqual(candidate.feedbackFrom, {continuationId:continuation.id, sourceCandidateId:first.candidate.id});
  assert.equal((await c.command('evidence', {target:f.run.id})).ready, false);
  assert.ok(nextRecord.checks.every((r:any) => r.candidateId !== candidate.id));
  assert.ok(nextRecord.reviews.every((r:any) => r.candidateId !== candidate.id));
  await assert.rejects(f.tool(coordinator, 'publication/prepare', {candidateId:candidate.id, summary:'Premature publication must be refused until fresh checks and independent review pass.'}), /locally validated/);
  await f.input(reviewer, {kind:'tool', name:'run_check', toolCallId:'feedback-check', arguments:{checkId:'required'}}, 'feedback-check-prompt');
  await until(async () => (await record()).checks.some((r:any) => r.candidateId === candidate.id), 45000);
  await f.idle();
  const check = (await record()).checks.find((r:any) => r.candidateId === candidate.id);
  assert.equal(check.completed, true); assert.equal(check.exitCode, 0); assert.equal(check.sourceIntact, true);
  await f.tool(coordinator, 'coding/review-assign', {assignee:next.reviewerName, candidateId:candidate.id, instruction:'Review the feedback addition at its exact revision.'});
  await f.tool(reviewer, 'coding/review', {candidateId:candidate.id, verdict:'pass', summary:'Fresh candidate and feedback case verified.', findings:[],
    coverage:[{criterion:0, assessment:'The preserved check and new feedback artifact cover the requirement.', checkIds:['required']}]});
  assert.equal((await c.command('evidence', {target:f.run.id})).ready, true);
  await f.tool(coordinator, 'publication/prepare', {candidateId:candidate.id, summary:'Validated native Resume successor addresses the captured reviewer feedback.'});
  await c.request(`/api/runs/${f.run.id}/publication/reconcile`, {});
  await paused();
  const final = await publication();
  assert.equal(final.ready, true); assert.equal(final.id, initial.id); assert.equal(final.pr.id, initial.pr.id);
  assert.equal(final.branch, initial.branch); assert.equal(final.candidateId, candidate.id);
  assert.equal(final.history[0].candidateId, first.candidate.id);
  assert.equal(posts, 1); assert.equal(puts, 1); assert.equal(reference(final.branch), candidate.revision.head);
  await ui('verify', {candidateId:candidate.id, previousCandidateId:first.candidate.id});
  assert.equal(execFileSync('git', ['-C', f.source, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim(), sourceHead);
  assert.equal(execFileSync('git', ['-C', f.source, 'status', '--porcelain'], {encoding:'utf8'}), '');
  // Source checkout remained untouched by publication. This explicit fixture-only
  // fast-forward below models the operator updating the dependency's checkout.
  await running(waiting.id); await c.request(`/api/runs/${waiting.id}/cancel`,{});
  let dependent=await queue('Wait for provider-observed merge',f.task.id);
  await operator('queue-inspect',{taskId:dependent.taskId,runId:dependent.id,slotRunId:null,waitingRunId:dependent.id,holdText:f.task.key});
  assert.equal((await c.request(`/api/runs/${dependent.id}/runtime`)).attempts.length,0);
  prs[0].state='MERGED'; prs[0].merge_commit={hash:candidate.revision.head};
  execFileSync('git',['-C',remote,'update-ref','refs/heads/main',candidate.revision.head],{stdio:'ignore'});
  await operator('reconcile-merge');
  const held=await until(async()=>{const w=await c.request('/api/workspace');return w.queue.plans.find((p:any)=>p.id===dependent.id)?.dependencyBaseHold;});
  assert.match(held.reason,/target|merge|branch/i);
  await operator('queue-inspect',{taskId:dependent.taskId,runId:dependent.id,slotRunId:null,waitingRunId:dependent.id,holdText:held.reason});
  execFileSync('git',['-C',f.source,'fetch',remote,'main'],{stdio:'ignore'});
  execFileSync('git',['-C',f.source,'merge','--ff-only','FETCH_HEAD'],{stdio:'ignore'});
  // The MH5 source gate retains the old queued plan until an explicit new acceptance and launch.
  await c.request(`/api/queue/${dependent.id}/remove`,{requestId:randomUUID()});
  await acceptFixturePlan(c,dependent.taskId);
  const task=await c.command('task',{target:dependent.taskId});
  dependent=await c.request(`/api/tasks/${dependent.taskId}/runs`,{revision:task.revision,launch:{repository:{mode:'new'},team:[f.team[0]]}});
  await running(dependent.id);
  await operator('queue-inspect',{taskId:dependent.taskId,runId:dependent.id,slotRunId:dependent.id});
  assert.equal(posts,1); assert.equal(puts,1);
  t.diagnostic(JSON.stringify({nativeReviewRelease:true,nativeWaitingHold:true,nativeQueuedResume:true,nativeMergedDependencyRelease:true,fixtureAdvancedCheckoutAfterUnchangedAssertion:true}));
  t.diagnostic(JSON.stringify({posts, puts, candidates:2, checks:(await record()).checks.length,
    reviews:(await record()).reviews.length, attempts:(await f.snapshot()).attempts.length,
    samePublication:final.id === initial.id, sameBranch:final.branch === initial.branch, sourceCheckoutUnchangedByPublication:true}));
});
