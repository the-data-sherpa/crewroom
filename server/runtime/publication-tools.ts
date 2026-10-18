export function publicationTools(coordinator: boolean) {
  return [{ name: 'publication_status', label: 'Inspect PR delivery', loadMode: 'essential',
    description: 'Read durable push/PR receipts and exact revision validation. Created PR and ready for human review are separate facts. A pending or uncertain action is not confirmed delivery.',
    parameters: { type: 'object', properties: {}, additionalProperties: false } },
  ...(coordinator ? [{ name: 'publish_change', label: 'Request PR publication', loadMode: 'essential',
    description: "Coordinator only. Persist a request to publish the locally validated current candidate. Crewroom pushes its owned branch, creates or reconciles one PR and observes hosted checks. The summary becomes the PR description a human reviewer reads first: what changed and why, the files touched, how it was verified (including the regression that fails before and passes after), risks, and anything left out. Write it for a reviewer, not as internal notes; Crewroom appends its evidence section. Calling again for the same candidate with a revised summary updates the PR description. Returns a queued receipt; inspect publication_status for outcomes. Human merge remains outside automatic execution.",
    parameters: { type: 'object', properties: { candidateId: { type: 'string' }, summary: { type: 'string', description: 'Markdown PR description for the human reviewer (40 to 20,000 characters).' } }, required: ['candidateId', 'summary'], additionalProperties: false } }] : [])];
}
