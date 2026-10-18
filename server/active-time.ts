import type { PublicationRecord } from '../shared/publication';

/** A matching provider observation establishes CI wait; quiet agents alone never do. */
export function hostedCiPending(record: PublicationRecord | undefined): boolean {
  if (!record?.observation || record.ready || record.error || record.merged || record.push.state !== 'confirmed' || record.pr.state !== 'confirmed'
    || record.remoteSourceHead !== record.revision.head || record.remoteTargetHead !== record.revision.base) return false;
  const observed=record.observation;
  if (!observed || observed.pr.state !== 'OPEN' || record.pr.providerState !== 'OPEN' || observed.pr.id !== record.pr.id
    || observed.pr.source.commit.hash !== record.revision.head || observed.pr.destination.commit.hash !== record.revision.base
    || observed.pr.source.branch.name !== record.branch || observed.pr.destination.branch.name !== record.target) return false;
  const pipeline=observed.pipelines[0];
  if (pipeline && (pipeline.target.commit.hash !== record.revision.head || pipeline.target.destination_commit?.hash !== record.revision.base
    || pipeline.target.pullrequest?.id !== record.pr.id)) return false;
  const pending=['PENDING','IN_PROGRESS'];
  if (pipeline && !pending.includes(pipeline.state.name) && !(pipeline.state.name==='COMPLETED' && pipeline.state.result?.name==='SUCCESSFUL')) return false;
  const statuses=record.policy.statusKeys.map(key=>observed.statuses.find(status=>status.key===key));
  if (statuses.some(status=>status && (!['SUCCESSFUL','INPROGRESS'].includes(status.state) || (status.refname && status.refname!==record.branch)))) return false;
  if (pipeline?.steps.some(step=>![...record.policy.pipelineSteps,...record.policy.optionalSteps].includes(step.name)
    || (record.policy.pipelineSteps.includes(step.name) && step.state.name==='COMPLETED' && step.state.result?.name!=='SUCCESSFUL'))) return false;
  return !!pipeline && pending.includes(pipeline.state.name) || statuses.some(status=>status?.state==='INPROGRESS');
}
