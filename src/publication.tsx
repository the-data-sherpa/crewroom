import type { PublicationRecord, FeedbackContinuation } from '../shared/publication';
import type { Action } from './main';

export function PublicationPanel({ runId, available, record, continuations, action, busy }: { runId: string; available: boolean; record: PublicationRecord; continuations: FeedbackContinuation[]; action?: Action; busy: boolean }) {
  return <section className="publication" aria-label="PR delivery">
    <div className="section-title"><h3>PR delivery</h3><strong className={`state-pill ${record.ready || record.merged ? 'validated' : ''}`}>{record.merged ? 'Merged' : record.ready ? 'Ready for human review' : 'Validation pending or blocked'}</strong></div>
    {record.merged && <p className="hash">Merge commit: {record.merged.commit} · observed {new Date(record.merged.observedAt).toLocaleString()}</p>}
    <p>Push: {record.push.state}</p><p>PR: {record.pr.state}</p>
    {record.pr.url && <p><a href={record.pr.url} target="_blank" rel="noreferrer">Open pull request</a></p>}
    <p className="hash">Published candidate: {record.revision.head}</p>
    {record.blockers.length > 0 && <ul aria-label="Publication blockers">{record.blockers.map(reason => <li key={reason}>{reason}</li>)}</ul>}
    {record.observation && <><p className="muted">Provider observations: {new Date(record.observation.observedAt).toLocaleString()}. Merge remains your decision.</p>
      <h4>Hosted statuses</h4><ul>{record.policy.statusKeys.map(key => <li key={key}>{key} · {record.observation!.statuses.find(item => item.key === key)?.state ?? 'Unavailable'}</li>)}</ul>
      <h4>PR pipeline</h4>{!record.observation.pipelines.length && <p>No matching pipeline observed for this PR revision.</p>}
      {record.observation.pipelines.map(pipeline => <div key={pipeline.uuid}><p>Build {pipeline.build_number} · {pipeline.state.result?.name ?? pipeline.state.name}</p>
        <p className="hash">Head: {pipeline.target.commit.hash}<br />Base: {pipeline.target.destination_commit?.hash ?? 'Unavailable'}</p>
        <ul>{pipeline.steps.map(step => <li key={step.uuid}>{step.name} · {step.state.result?.name ?? step.state.name}</li>)}</ul></div>)}</>}
    {record.pr.id && <section aria-label="PR feedback">
      <h4>PR feedback · #{record.pr.id}</h4>
      <p className="muted">Source: {record.repository.workspace}/{record.repository.slug} · repository {record.repository.uuid} · PR #{record.pr.id}</p>
      {!record.feedback ? <p>Feedback has not been observed yet. This is not a confirmed empty review.</p> : <>
        <p>Feedback read: {record.feedback.state} · {new Date(record.feedback.observedAt).toLocaleString()}{record.feedback.reason ? ` · ${record.feedback.reason}` : ''}</p>
        {!record.feedback.comments.length && <p>{record.feedback.state === 'complete' ? 'No PR comments observed.' : 'No feedback is known; the read is incomplete.'}</p>}
        {!!record.feedback.comments.length && <ul>{record.feedback.comments.map(comment => {
          const captures = continuations.filter(item => item.publicationId === record.id && item.prId === record.pr.id
            && item.feedback.comments.some(saved => saved.id === comment.id));
          return <li key={comment.id}><strong>Comment #{comment.id}</strong> · {comment.user?.display_name ?? comment.user?.uuid ?? 'Author unavailable'}
            {' '}· updated {new Date(comment.updated_on).toLocaleString()}{comment.inline && <> · {comment.inline.path}:{comment.inline.to ?? comment.inline.from ?? 'file'}</>}
            {comment.parent && <> · reply to #{comment.parent.id}</>}
            {comment.inline?.outdated && <> · outdated line</>}{comment.deleted && <> · deleted at source</>}
            {comment.resolution && <> · resolved</>}{comment.currentness !== 'known' && <> · {comment.currentness === 'conflict' ? 'conflicting version; prior text retained' : 'currentness unknown'}</>}
            {comment.content?.raw && <p>{comment.content.raw}</p>}
            {captures.map(item => {
              const saved = item.feedback.comments.find(entry => entry.id === comment.id)!;
              return <p className="muted" key={item.id}>Captured by continuation {item.id} · comment version {new Date(saved.updated_on).toLocaleString()}
                {saved.updated_on !== comment.updated_on && <> · earlier than the current version. Captured text: {saved.content?.raw ?? 'Unavailable'}</>}
                {' '}· {item.feedback.state} read · {item.state.replace('_', ' ')}</p>;
            })}
          </li>;
        })}</ul>}
      </>}
    </section>}
    {action && <button disabled={busy || !available} onClick={() => action(`/runs/${runId}/publication/reconcile`, {}, 'Publication receipts refreshed.')}>Reconcile publication</button>}
  </section>;
}
