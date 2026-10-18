import type { AttemptUsage, RunUsage } from '../server/run-usage';

export function UsageNumbers({ usage }: { usage: Pick<AttemptUsage, 'state' | 'tokens' | 'actualResponses' | 'providerRequests' | 'costUsd' | 'costResponses' | 'responses'> & Partial<Pick<AttemptUsage, 'awaitingStart' | 'requestCounts' | 'costBasis'>> }) {
  return <>
    {usage.awaitingStart && <p>Not started · excluded from run total.</p>}
    <p className="usage-tokens">Provider tokens · {usage.state}{usage.tokens !== null && ` · ${usage.tokens.total?.toLocaleString() ?? 'unavailable'} recorded`}</p>
    {usage.tokens && <p className="muted">{usage.tokens.input?.toLocaleString() ?? 'unavailable'} in ({usage.tokens.cacheRead?.toLocaleString() ?? 'unavailable'} cached) · {usage.tokens.output?.toLocaleString() ?? 'unavailable'} out</p>}
    <p className="muted">{usage.requestCounts === 'unavailable' ? 'Provider request counts unavailable.' : `${usage.actualResponses} / ${usage.providerRequests} provider requests reported usage.${usage.requestCounts === 'partial' ? ' Partial counts; native harness requests unavailable.' : ''}`}</p>
    <p>Runner cost estimate (USD): {usage.costUsd === null ? 'unavailable' : `$${usage.costUsd.toFixed(5)}${usage.state !== 'actual' || usage.costResponses !== usage.responses ? ' · partial' : ''}`}<small>{usage.costBasis === 'cumulative-list-price-estimate' ? 'Includes cumulative list-price estimates' : usage.requestCounts === 'unavailable' ? 'Response cost coverage unavailable' : `Runner pricing · ${usage.costResponses} / ${usage.responses} responses with cost`} · Provider billing and subscription charges unavailable</small></p>
  </>;
}
export function UsagePanel({ usage }: { usage: RunUsage }) {
  return <details className="usage"><summary>Recorded usage and cost</summary>
    <section aria-label="Run usage"><h3>Run total</h3><UsageNumbers usage={usage.total} />
      {!!usage.total.incompleteAttempts.length && <p>Incomplete usage: {usage.total.incompleteAttempts.map(item => `${item.name} attempt ${item.generation}`).join(', ')}.</p>}
      <p className="muted">Source: {usage.source}.</p><p className="muted">{usage.note}</p>
    </section>
    {!usage.rows.length ? <p>No agent attempts recorded.</p> : <ul className="usage-attempts">{usage.rows.map(row => <li key={row.attemptId}>
      <h4>{row.name} · attempt {row.generation}</h4><p className="muted">{row.model}</p><UsageNumbers usage={row} />
      {row.unavailableReasons.map(reason => <p key={reason} className="muted">{reason}</p>)}
    </li>)}</ul>}
  </details>;
}
