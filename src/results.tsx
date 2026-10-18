import { useEffect, useState } from 'react';
import { useLiveStatus, useTopic } from './live';
import type { Results } from '../server/results';
import type { Candidate } from '../shared/evidence';
import type { Action } from './main';
import type { Workspace } from '../shared/types';
import { PublicationPanel } from './publication';
import { Markdown } from './markdown';
import { UsagePanel } from './usage';
type ResultView = Awaited<ReturnType<Results['read']>>;
type Load = (path: string) => Promise<any>;
const short = (value: string) => value.slice(0, 12);
type CheckReceipt = ResultView['checks'][number];
const refreshLabels = { rebasing: 'Git recovery unverified · operator inspection required', completed: 'Rebased onto target', conflicted: 'Conflicted · branch unchanged', interrupted: 'Interrupted · branch unchanged' } as const;
/** A check that never completed is interrupted, not failed: only a completed run with a non-zero exit fails. */
export function checkState(check: CheckReceipt | undefined, stale: boolean | undefined, passed: boolean | undefined) {
  if (!check) return 'missing' as const;
  if (stale) return 'stale' as const;
  if (passed) return 'passed' as const;
  if (check.timedOut) return 'timed_out' as const;
  if (!check.started || !check.completed) return 'interrupted' as const;
  if (!check.sourceIntact) return 'source_changed' as const;
  return 'failed' as const;
}
const checkLabels = { missing: 'Not run', stale: 'Out of date', passed: 'Passed', timed_out: 'Timed out', interrupted: 'Interrupted · incomplete', source_changed: 'Source changed during check', failed: 'Failed' };

export function ResultPanel({ runId, load, action, busy = false, repositories }: { runId: string; load: Load; action?: Action; busy?: boolean; repositories?: Workspace['repositories'] }) {
  const { data: result } = useTopic<ResultView>(`result:${runId}`);
  const connected = useLiveStatus() === 'open';
  const error = connected || !result ? '' : 'Evidence may be out of date. Reconnecting…';
  const [selected, setSelected] = useState('');
  const [retryReason, setRetryReason] = useState('');
  const [refreshOutcome, setRefreshOutcome] = useState('');
  const operatorHold = result?.refreshes.findLast(item => item.state === 'conflicted' && !item.supersededBy);
  const operatorRetry = !!operatorHold && (!operatorHold.resolutionWorkspace || operatorHold.resolutionWorkspace.state === 'operator_required');
  useEffect(() => setRetryReason(''), [runId, operatorHold?.requestId]);
  useEffect(() => {
    if (result?.publication.record?.pr.id) void load(`/runs/${runId}/feedback`).catch(() => {});
  }, [runId, result?.publication.record?.pr.id, load]);
  const latest = result?.candidates.at(-1); const candidate = result?.candidates.find(item => item.id === selected) ?? latest;
  const currentRepository = repositories?.find(item => item.id === result?.repository?.id);
  const configurationChanged = !!result?.repository && !!currentRepository && JSON.stringify([
    result.repository.name, result.repository.path, result.repository.targetBranch, result.repository.branchPrefix, result.repository.checks,
  ]) !== JSON.stringify([currentRepository.name, currentRepository.path, currentRepository.targetBranch, currentRepository.branchPrefix, currentRepository.checks]);
  if (result?.research) return <><ResearchEvidence runId={runId} research={result.research} load={load} error={error} /><UsagePanel usage={result.usage} /></>;
  return <section className="card result" aria-label="Change and evidence">
    <div className="section-title"><h2>Change and evidence</h2>{result && !error && <span className={`state-pill ${result.assessment.ready ? 'validated' : ''}`}>{result.assessment.ready ? 'Locally validated' : 'Validation incomplete'}</span>}</div>
    {error && <p role="alert" className="error">{error}</p>}
    {!result && <p>Loading saved evidence…</p>}
    {result && <>
      <p className="muted">{result.publication.reason}</p>
      {result.repository && <section className="target-details" aria-label="Run repository snapshot"><h3>Run repository · captured at preparation</h3>
        <p><strong>{result.repository.name}</strong> · <span className="mono">{result.repository.path}</span></p>
        <p>Target: {result.repository.targetBranch} · Prefix: {result.repository.branchPrefix ?? 'codex/crewroom/'}</p>
        <p>Checks: {(result.repository.checks ?? []).map(check => check.id).join(', ') || 'none'}</p>
        {configurationChanged && <p className="muted">Current repository configuration differs. These are the run's captured settings, not the latest configuration.</p>}
      </section>}
      {result.publication.record && <PublicationPanel runId={runId} available={result.publication.available} record={result.publication.record} continuations={result.feedbackContinuations ?? []} action={action} busy={busy} />}
      {!!result.assessment.blockers.length && <ul className="evidence-blockers" aria-label="Readiness blockers">{result.assessment.blockers.map(reason => <li key={reason}>{reason}</li>)}</ul>}
      {!!result.checkPolicyChanges.length && <details><summary>Check environment changes</summary><ul>{result.checkPolicyChanges.map(change => <li key={change.requestId}><strong>{change.checkId} · Revision {change.revision}</strong><p>{change.reason}</p>{change.image !== change.expectedImage && <><p className="hash">Previous image: {change.expectedImage}</p><p className="hash">Replacement image: {change.image}</p></>}{change.memoryMiB !== undefined && <p>Memory limit: {change.expectedMemoryMiB} MiB → {change.memoryMiB} MiB</p>}</li>)}</ul></details>}
      {!!result.refreshes.length && <details open={result.refreshes.at(-1)?.state !== 'completed'}><summary>Target refreshes</summary><ul aria-label="Target refreshes">{result.refreshes.map(item => <li key={item.requestId}>
        <strong>{item.resolutionWorkspace?.legacyOwnershipUnknown ? 'Legacy Git ownership unverified · operator inspection required' : item.supersededBy ? item.resolutionWorkspace ? 'Superseded · prior workspace retained' : 'Superseded · prior conflict retained' : ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '') ? 'Git ownership unverified · operator inspection required' : item.holdReason === 'interrupted_retry' ? 'Retry interrupted · operator decision required' : item.state === 'completed' && item.fromBase === item.toBase && item.fromHead === item.toHead ? item.supersedes ? 'Current target verified · no rebase needed' : 'Target unchanged · no rebase needed' : item.state === 'conflicted' && item.resolutionWorkspace?.state === 'ready' ? 'Resolved · awaiting promotion' : refreshLabels[item.state]}</strong> · {new Date(item.at).toLocaleString()}
        {result.completions.filter(completion => completion.conflictRefreshId === item.requestId && completion.state === 'rejected' && completion.rejection).map(completion => {
          const rejection = completion.rejection!;
          return <p className="callout" key={completion.id}>Conflict completion rejected: {rejection.reason} {rejection.code === 'target_changed'
            ? <>Target {short(rejection.expectedTarget)} → {short(rejection.observedTarget)} observed {new Date(rejection.at).toLocaleString()}.</>
            : <>Checked reset did not apply; retry is available even if the target remains {short(rejection.expectedTarget)}. Recorded {new Date(rejection.at).toLocaleString()}.</>}
            {' '}The resolved files and isolated workspace are retained at {item.resolutionWorkspace?.path}; no candidate was created. {item.supersededBy ? 'A reasoned newer attempt superseded this hold.' : result.refreshable ? 'Give a reason below to retry against the current target.' : 'Keep agents stopped; retry requires reconciled ownership and publication.'} This completion cannot be replayed.</p>;
        })}
        <p className="hash">{item.holdReason === 'interrupted_retry' ? 'Attempted base (historical): ' : 'Base: '}{short(item.fromBase)} → {short(item.toBase)}{item.toHead ? ` · Head: ${short(item.fromHead)} → ${short(item.toHead)}` : ''}</p>
        {(item.state === 'rebasing' || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')) && <p className="callout">Git mutation ownership or recovery is unverified. Retain the original branch and any isolated workspace for operator inspection. Do not resume agents, retry, edit, check, or publish; the attempted target is not a verified candidate.</p>}
        {item.state === 'completed' && item.supersedes && item.fromBase === item.toBase && item.fromHead === item.toHead && <p className="callout">The reasoned retry verified the unchanged target and closed the prior hold. No rebase or new candidate was needed; the retry did not resume the run or change evidence gates.</p>}
        {item.state === 'completed' && !item.supersedes && item.fromBase === item.toBase && item.fromHead === item.toHead && <p className="callout">The target was unchanged. The existing candidate was retained; no rebase or new candidate was needed.</p>}
        {!!item.conflicts?.length && <p className="callout">{item.resolutionWorkspace?.state === 'ready' || (item.resolutionWorkspace?.state === 'operator_required' && !!item.resolutionWorkspace.toHead) ? 'Resolved paths:' : 'Conflicts:'} {item.conflicts.join(', ')}</p>}
        {item.state === 'conflicted' && item.resolutionWorkspace?.state === 'conflicted' && <p className="callout">Resolution workspace retained. Resume the run to let the coordinator assign an in-scope specialist; the original branch and PR remain unchanged until promotion.</p>}
        {item.state === 'conflicted' && item.resolutionWorkspace?.state === 'operator_required' && !result.completions.some(completion => completion.conflictRefreshId === item.requestId && completion.state === 'rejected') && <p className="callout">Operator decision required: the conflict affects project instructions, exceeds confirmed scope or its Git state changed. The original branch and PR remain unchanged.</p>}
        {item.holdReason === 'interrupted_retry' && <p className="callout">This retry stopped before target verification completed. The prior attempt and any retained workspace remain in history. Keep agents stopped and give a reason to start a new attempt against the current target. No conflict resolution workspace or candidate was created by this retry.</p>}
        {item.state === 'conflicted' && !item.resolutionWorkspace && !item.holdReason && <p className="callout">Operator decision required: rebasing would change the recorded source scope or the target already contains part of this candidate. The original branch and PR remain unchanged.</p>}
        {item.state === 'conflicted' && item.resolutionWorkspace?.state === 'promoting' && <p className="callout">Promotion outcome is uncertain until managed Git mutation ownership and the original branch are confirmed. Do not resume agents or retry the target from this state.</p>}
        {item.state === 'conflicted' && item.resolutionWorkspace?.state === 'ready' && <p className="callout">The isolated rebase is ready. Submit implementation completion to create a candidate; previous checks and review do not cover it.</p>}
        {item.supersededBy && <p className="callout">An operator chose a fresh target attempt: {item.supersededBy.reason}. {item.resolutionWorkspace ? 'The isolated workspace remains in history.' : 'The prior conflict remains in history.'}</p>}
        </li>)}</ul></details>}
      {action && result.refreshable && <>
        {operatorRetry && <label>Reason for a fresh target attempt<textarea rows={2} maxLength={4000} value={retryReason} onChange={event => setRetryReason(event.target.value)} /></label>}
        <p><button disabled={busy || (operatorRetry && !retryReason.trim())} onClick={async () => {
          const payload = { requestId: crypto.randomUUID(), ...(operatorRetry && operatorHold ? { operatorDecision: { conflictRequestId: operatorHold.requestId, reason: retryReason.trim() } } : {}) };
          const saved = await action(`/runs/${runId}/target-refresh`, payload, 'Target request completed. See refresh history for the verified outcome.');
          if (saved) {
            setRefreshOutcome(saved.refresh?.state === 'completed' && !saved.changed ? operatorRetry ? 'Current target verified; retry recorded without rebasing or creating a candidate.' : 'Target unchanged; the existing candidate was retained without rebasing.'
              : saved.refresh?.state === 'conflicted' ? 'A target conflict remains held; inspect the refresh history.'
                : saved.changed ? 'The owned branch was rebased; the new candidate needs its checks and review.' : 'Target verified; no branch change was needed.');
            if (operatorRetry) setRetryReason('');
          }
        }}>{operatorRetry ? 'Retry from current target' : 'Refresh from target'}</button>
          <small className="muted">{operatorRetry ? 'Verifies the current target; a new isolated attempt is used only if a rebase is needed. The original branch, PR, and prior conflict workspace remain unchanged until a successful rebase or promotion.' : 'Checks the current target; rebases only when needed. A new candidate needs its own checks and review.'}</small></p>
        {refreshOutcome && <p role="status">{refreshOutcome}</p>}
      </>}
      {operatorHold?.holdReason === 'interrupted_retry' && candidate && <p className="callout">The candidate below predates the interrupted retry; it is not current-target verification or publication readiness.</p>}
      {result.refreshes.some(item => item.state === 'rebasing' || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')) && candidate && <p className="callout">The candidate below predates unverified Git work and does not establish current-target readiness.</p>}
      {candidate && <>
        <label>Candidate revision<select value={candidate.id} onChange={event => setSelected(event.target.value === latest?.id ? '' : event.target.value)}>{result.candidates.map((item, index) => <option key={item.id} value={item.id}>Revision {index + 1} · {short(item.revision.head)}{item.id === latest?.id ? ' · Latest' : ''}</option>)}</select></label>
        {candidate.id !== latest?.id && <p className="callout">Historical revision</p>}
        {candidate.refreshedFrom && <p className="muted">Rebased from revision {result.candidates.findIndex(item => item.id === candidate.refreshedFrom) + 1} onto a newer target.</p>}
        <CandidateEvidence key={candidate.id} runId={runId} candidate={candidate} result={result} load={load} />
      </>}
      <UsagePanel usage={result.usage} />
    </>}
  </section>;
}
function ResearchEvidence({ runId, research, load, error }: { runId: string; research: NonNullable<ResultView['research']>; load: Load; error: string }) {
  return <section className="card result" aria-label="Research outcome and evidence">
    <div className="section-title"><h2>Research outcome and evidence</h2><span className={`state-pill ${research.assessment.completed ? 'validated' : ''}`}>{research.assessment.completed ? 'Completed research outcome' : 'Research incomplete'}</span></div>
    {error && <p role="alert" className="error">{error}</p>}
    <p className="muted">Accepted revision {research.assessment.revision}{research.source ? ` · Pinned source ${research.source.commit}` : ' · No repository assigned'}. This outcome has no coding gates, commit or PR.</p>
    {research.outcome && !research.assessment.completed && <p className="callout">Previous completion for revision {research.outcome.revision} is stale after adoption; current criteria need new verified artifacts.</p>}
    {!!research.assessment.blockers.length && <ul className="evidence-blockers" aria-label="Research blockers">{research.assessment.blockers.map(reason => <li key={reason}>{reason}</li>)}</ul>}
    <h3>Accepted criterion coverage</h3><ul>{research.criteria.map((criterion, id) => <li key={id}>Criterion {id}: {criterion} · {research.assessment.covered.includes(id) ? 'Verified' : 'Uncovered'}</li>)}</ul>
    {!research.artifacts.length && <p>No artifacts submitted.</p>}
    {research.artifacts.map(item => <ResearchArtifactView key={item.id} runId={runId} item={item} currentRevision={research.assessment.revision} pinnedCommit={research.source?.commit} load={load} />)}
  </section>;
}

function ResearchArtifactView({ runId, item, currentRevision, pinnedCommit, load }: { runId: string; item: NonNullable<ResultView['research']>['artifacts'][number]; currentRevision: number; pinnedCommit?: string; load: Load }) {
  const [body, setBody] = useState<string>(); const [error, setError] = useState('');
  useEffect(() => { let active = true; void load(`/runs/${runId}/result/artifacts/${item.body.id}`).then(value => { if (active) setBody(value.content); }, cause => { if (active) setError(cause instanceof Error ? cause.message : 'Artifact is unavailable.'); }); return () => { active = false; }; }, [runId, item.body.id, load]);
  return <article className="artifact" aria-label={`${item.title} artifact`}><h3>{item.title} · {item.kind}</h3>
    <p className="muted">{item.state === 'rejected' ? 'Rejected' : item.supersededBy ? 'Superseded' : item.requirementRevision !== currentRevision || item.pinnedCommit !== pinnedCommit ? 'Stale · accepted revision changed' : 'Verified'} · submitted {new Date(item.createdAt).toLocaleString()}</p>
    {item.reasons.length > 0 && <ul className="evidence-blockers">{item.reasons.map((reason, id) => <li key={id}>{reason}</li>)}</ul>}
    {item.supersededBy && <p>Superseded by artifact {item.supersededBy}.</p>}
    <p className="hash">SHA-256 {item.body.sha256} · {item.body.bytes} bytes</p>
    <p className="muted">Assignment {item.assignmentId} · attempt {item.attemptId} · model {item.model.selector} ({item.model.effort}) requested {item.model.requested.model} ({item.model.requested.effort}){item.model.fallbackFrom ? ` · fallback from ${item.model.fallbackFrom}` : ''} · requirement revision {item.requirementRevision} · pinned commit {item.pinnedCommit ?? 'none'}</p>
    {error && <p role="alert" className="error">{error}</p>}{body === undefined && !error ? <p>Loading artifact…</p> : body !== undefined && <Markdown text={body} />}
    <h4>Citations</h4>{!item.citations.length ? <p>No repository citations.</p> : <ul>{item.citations.map((citation, id) => <li key={id}><strong>{citation.verified ? 'Verified citation' : 'Rejected citation'}</strong> · {citation.path}:{citation.startLine}-{citation.endLine} @ {citation.commit}<p>{citation.excerpt}</p>{citation.reason && <p className="callout">{citation.reason}</p>}</li>)}</ul>}
    <h4>Limitations</h4><p>{item.limitations || 'Missing limitations'}</p>
    <h4>Criterion coverage</h4><ul>{item.coverage.map(entry => <li key={entry.criterionId}>Criterion {entry.criterionId} → {entry.section}</li>)}</ul>
  </article>;
}


function CandidateEvidence({ runId, candidate, result, load }: { runId: string; candidate: Candidate; result: ResultView; load: Load }) {
  const [content, setContent] = useState<{ label: string; body: string; hash?: string }>(); const [diff, setDiff] = useState<string>(); const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const checks = result.checks.filter(item => item.candidateId === candidate.id);
  const reviews = [...new Map(result.reviews.filter(item => item.candidateId === candidate.id).map(item => [item.assignmentId, item])).values()];
  const producer = (attemptId: string) => { const attempt = result.attempts.find(item => item.id === attemptId); return `${result.assignments.find(item => item.id === attempt?.assignmentId)?.name ?? 'Unknown assignment'} · attempt ${short(attemptId)} · ${attempt?.model.resolved.selector ?? 'model unavailable'} · session ${attempt?.sessionId ?? 'unavailable'}`; };
  const artifact = async (id: string, label: string) => {
    setLoading(true); setError(''); try { const value = await load(`/runs/${runId}/result/artifacts/${id}`); setContent({ label, body: value.content, hash: value.sha256 }); }
    catch (error) { setError(error instanceof Error ? error.message : 'Evidence is unavailable.'); } finally { setLoading(false); }
  };
  return <>
    <dl className="revision"><div><dt>Base</dt><dd>{candidate.revision.base}</dd></div><div><dt>Candidate commit</dt><dd>{candidate.revision.head}</dd></div><div><dt>Tree</dt><dd>{candidate.revision.tree}</dd></div></dl>
    <button disabled={loading} onClick={async () => { setLoading(true); setError(''); try { const value = await load(`/runs/${runId}/result/candidates/${candidate.id}/diff`); setDiff(value.patch || 'No file changes from the recorded base.'); } catch (error) { setError(error instanceof Error ? error.message : 'Change preview unavailable.'); } finally { setLoading(false); } }}>View change</button>
    {diff !== undefined && <pre className="evidence-content" aria-label="Candidate diff">{diff}</pre>}
    <h3>Repository checks</h3>
    {!result.requiredChecks.length && <p>No required checks configured.</p>}
    <ul className="evidence-list">{result.requiredChecks.map(definition => {
      const check = checks.findLast(item => item.checkId === definition.id); const passed = check && check.started && check.completed && check.sourceIntact && !check.timedOut && check.exitCode === 0;
      const stale = check && JSON.stringify(check.definition) !== JSON.stringify(definition);
      return <li key={definition.id}><div className="section-title"><strong>{definition.id}{definition.required ? ' · Required' : ' · Optional'}</strong><span className={`state-pill check-${checkState(check, stale, passed)}`}>{checkLabels[checkState(check, stale, passed)]}</span></div>
        <p className="command"><code>{JSON.stringify([definition.command, ...definition.args])}</code></p>
        {check && <><p>{check.timedOut ? 'Timed out' : !check.completed ? 'Execution incomplete' : `Exit ${check.exitCode}`} · {check.environment.platform} · Network {check.environment.network}{check.environment.memoryMiB !== undefined ? ` · Memory limit ${check.environment.memoryMiB} MiB` : ''}</p><p className="muted">Source snapshot {check.sourceIntact ? 'verified' : 'changed or unavailable'} · {producer(check.attemptId)}</p>
          <details><summary>Check identity and artifacts</summary><p className="hash">Image: {check.environment.image}</p>{check.environment.tmpExecutable !== undefined && <p>Temporary file execution: {check.environment.tmpExecutable ? 'enabled' : 'disabled'}</p>}<p className="hash">Output SHA-256: {check.output.sha256}</p><p className="hash">Source SHA-256: {check.source.sha256}</p></details>
          <button disabled={loading} onClick={() => void artifact(check.output.id, `${definition.id} output`)}>View check output</button></>}
      </li>;
    })}</ul>
    {result.checkOperations.filter(item => item.candidateId === candidate.id && item.state === 'rejected').map(item => <p className="callout" key={item.id}>{item.definition.id}: {item.reason}</p>)}
    <h3>Independent review</h3>
    {!reviews.length && <p>No independent review recorded for this revision.</p>}
    {reviews.map(review => <article className="review" key={review.id}><strong>{review.verdict === 'pass' ? 'Independent review passed' : 'Independent review requested changes'}</strong><p>{review.summary}</p><p className="muted">{producer(review.attemptId)}</p>
      {(review.checkPolicyRevision ?? 0) !== result.checkPolicyChanges.length && <p className="callout">Review predates the current check environment.</p>}
      {!!review.findings.length && <ul>{review.findings.map((finding, index) => <li key={index}><strong>{finding.severity}</strong>{finding.path && ` · ${finding.path}${finding.line ? `:${finding.line}` : ''}`}<p>{finding.body}</p></li>)}</ul>}
      <h4>Criteria coverage</h4><ul>{result.criteria.map((criterion, index) => { const coverage = review.coverage.find(item => item.criterion === index); return <li key={index}><strong>{criterion}</strong><p>{coverage ? coverage.assessment : 'No coverage recorded.'}{coverage?.checkIds.length ? ` Checks: ${coverage.checkIds.join(', ')}.` : ''}</p></li>; })}</ul>
      <button disabled={loading} onClick={() => void artifact(review.artifact.id, 'Review record')}>View review record</button>
    </article>)}
    {error && <p role="alert" className="error">{error}</p>}
    {content && <div className="artifact"><h3>{content.label}</h3><p className="hash">Verified SHA-256: {content.hash}</p><pre className="evidence-content" aria-label="Evidence content">{content.body}</pre></div>}
  </>;
}
