export function jiraTools(reporter: boolean, research = false, coordinatorFallback = false) {
  const kinds = research ? coordinatorFallback ? ['research_outcome'] : ['milestone', 'blocker', 'research_outcome'] : ['milestone', 'blocker', 'in_progress', 'evidence', 'pr_link'];
  const description = !research
    ? 'Delivery reporter only. Queue an allowed Jira update. Use a stable eventKey for the same milestone across retries and replacement sessions; inspect jira_updates first. in_progress requires eventKey execution-started. milestone/blocker require body. evidence/pr_link require the current verified candidateId; pr_link also requires a confirmed PR receipt and is composed by Crewroom. Do not rewrite requirements or claim delivery from a pending receipt.'
    : coordinatorFallback
      ? 'Research coordinator fallback only when no delivery reporter is assigned. After requesting verified completion, queue research_outcome with eventKey research-outcome-<accepted revision>; never a Jira transition.'
      : 'Research reporter only. After verified completion is requested, queue research_outcome with eventKey research-outcome-<accepted revision>. Milestone/blocker comments remain available; no Jira transitions, PR or coding evidence.';
  return [{ name: 'jira_updates', label: 'Inspect Jira delivery', loadMode: 'essential',
    description: 'Read durable Jira update receipts for this run. Pending or uncertain is not delivered. Jira status and workflow progress are separate facts.',
    parameters: { type: 'object', properties: {}, additionalProperties: false } },
  ...(reporter ? [{ name: 'jira_report', label: 'Queue Jira update', loadMode: 'essential',
    description,
    parameters: { type: 'object', properties: { kind: { type: 'string', enum: kinds }, eventKey: { type: 'string' }, body: { type: 'string' }, ...(research ? {} : { candidateId: { type: 'string' } }) }, required: ['kind', 'eventKey'], additionalProperties: false } }] : [])];
}
