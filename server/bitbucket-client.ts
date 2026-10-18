import { z } from 'zod';

const hash = z.string().regex(/^[a-f0-9]{40,64}$/);
const repositoryIdentity = z.object({ uuid: z.string().regex(/^\{[a-f0-9-]{36}\}$/i) });
export const bitbucketRepositorySchema = repositoryIdentity.extend({ workspace: z.string().regex(/^[a-zA-Z0-9_-]+$/), slug: z.string().regex(/^[a-zA-Z0-9_.-]+$/) }).strict();
export type BitbucketRepository = z.infer<typeof bitbucketRepositorySchema>;
/** Pull-request objects report 12-character commit hashes; they are expanded to full identities before use. */
const abbreviated = z.string().regex(/^[a-f0-9]{7,64}$/);
const ref = z.object({ branch: z.object({ name: z.string() }), commit: z.object({ hash: abbreviated }), repository: repositoryIdentity });
const prSchema = z.object({ id: z.number().int().positive(), title: z.string(), description: z.string(), state: z.enum(['OPEN', 'MERGED', 'DECLINED', 'SUPERSEDED']),
  updated_on: z.string().refine(value => Number.isFinite(Date.parse(value))), source: ref, destination: ref,
  links: z.object({ html: z.object({ href: z.url() }) }), merge_commit: z.object({ hash: abbreviated }).nullish(), draft: z.boolean().optional() });
export type BitbucketPullRequest = z.infer<typeof prSchema>;
const date = z.string().refine(value => Number.isFinite(Date.parse(value)));
const state = z.object({ name: z.string(), result: z.object({ name: z.string() }).optional() });
const statusSchema = z.object({ key: z.string(), state: z.string(), name: z.string(), url: z.url(), updated_on: date, refname: z.string().optional() });
const stepSchema = z.object({ uuid: z.string(), name: z.string(), state });
const pipelineSchema = z.object({ uuid: z.string(), build_number: z.number().int(), run_number: z.number().int().optional(), created_on: date, state,
  target: z.object({ type: z.string(), commit: z.object({ hash }), destination_commit: z.object({ hash: abbreviated }).optional(), source: z.string().optional(), destination: z.string().optional(),
    pullrequest: z.object({ id: z.coerce.number().int().positive() }).optional(), selector: z.object({ type: z.string(), pattern: z.string() }) }) });
const commentSchema = z.object({ id: z.number().int().positive(), created_on: date, updated_on: date, deleted: z.boolean().default(false),
  content: z.object({ raw: z.string().max(40000) }).optional(),
  user: z.object({ uuid: z.string().optional(), display_name: z.string().optional() }).optional(),
  parent: z.object({ id: z.number().int().positive() }).optional(),
  pullrequest: z.object({ id: z.number().int().positive() }).optional(),
  inline: z.object({ path: z.string().max(1000), from: z.number().int().positive().nullable().optional(), to: z.number().int().positive().nullable().optional(),
    start_from: z.number().int().positive().nullable().optional(), start_to: z.number().int().positive().nullable().optional(), outdated: z.boolean().optional() }).optional(),
  resolution: z.object({ created_on: date.optional(), user: z.object({ uuid: z.string().optional(), display_name: z.string().optional() }).optional() }).optional().nullable() });
export type BitbucketComment = z.infer<typeof commentSchema>;
export interface BitbucketCommentRead { state: 'complete' | 'partial' | 'unavailable'; comments: BitbucketComment[]; reason?: string }
export interface BitbucketObservation {
  observedAt: string; pr: BitbucketPullRequest; statuses: z.infer<typeof statusSchema>[];
  pipelines: (z.infer<typeof pipelineSchema> & { steps: z.infer<typeof stepSchema>[] })[];
}
/** A string is an OAuth/access bearer token; API tokens require an explicit account. */
export type BitbucketCredential = string | { scheme: 'basic'; username: string; token: string };
export interface BitbucketConnection { repository: BitbucketRepository; credentials: () => Promise<BitbucketCredential>; apiUrl?: string }
export class BitbucketUnavailable extends Error {
  constructor(message: string, readonly status?: number, readonly requestSent = false) { super(message); }
}

/** Fixed-origin, bounded Bitbucket REST calls. Mutations never receive transport retries. */
export class BitbucketClient {
  readonly repository: BitbucketRepository;
  private origin = 'https://api.bitbucket.org';
  private prefix: string;
  constructor(private connection: BitbucketConnection) {
    this.repository = bitbucketRepositorySchema.parse(connection.repository);
    this.prefix = `/2.0/repositories/${this.repository.workspace}/${this.repository.slug}`;
    if (connection.apiUrl) {
      const url = new URL(connection.apiUrl);
      if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.username || url.password || url.pathname !== '/' || url.search || url.hash) throw new Error('Fixture Bitbucket endpoints must use a loopback origin.');
      this.origin = url.origin;
    }
  }
  private async request(path: string, method = 'GET', body?: unknown, beforeDispatch?: () => void | Promise<void>): Promise<unknown> {
    let requestSent = false;
    try {
      const credential = await this.connection.credentials();
      const token = typeof credential === 'string' ? credential : credential.token;
      if (!token || /[\r\n]/.test(token)) throw new BitbucketUnavailable('Bitbucket credentials are unavailable.');
      if (typeof credential !== 'string' && (credential.scheme !== 'basic' || !credential.username || /[:\r\n]/.test(credential.username))) throw new BitbucketUnavailable('Bitbucket credentials are unavailable.');
      const authorization = typeof credential === 'string' ? `Bearer ${token}` : `Basic ${Buffer.from(`${credential.username}:${token}`).toString('base64')}`;
      await beforeDispatch?.();
      requestSent = true;
      const response = await fetch(this.origin + this.prefix + path, { method, redirect: 'error', signal: AbortSignal.timeout(15000),
        headers: { Authorization: authorization, Accept: 'application/json', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) }, body: body === undefined ? undefined : JSON.stringify(body) });
      if (!response.ok) { await response.body?.cancel(); throw new BitbucketUnavailable(`Bitbucket returned HTTP ${response.status}. Saved delivery intents are retained.`, response.status); }
      const chunks: Buffer[] = []; let bytes = 0;
      for await (const part of response.body ?? []) { bytes += part.length; if (bytes > 4 * 1024 * 1024) throw new BitbucketUnavailable('Bitbucket response exceeded its size limit.'); chunks.push(Buffer.from(part)); }
      const text = Buffer.concat(chunks).toString('utf8'); return text ? JSON.parse(text) : undefined;
    } catch (error) {
      if (error instanceof BitbucketUnavailable) throw new BitbucketUnavailable(error.message, error.status, requestSent);
      throw new BitbucketUnavailable('Bitbucket is unavailable. Saved delivery intents are retained.', undefined, requestSent);
    }
  }
  private async pages(path: string, onPage?: (values: unknown[]) => void): Promise<unknown[]> {
    const values: unknown[] = []; const visited = new Set<string>(); const endpoint = new URL(this.origin + this.prefix + path).pathname;
    for (;;) {
      if (visited.has(path) || visited.size >= 100) throw new BitbucketUnavailable('Bitbucket pagination did not complete.');
      visited.add(path);
      const page = z.object({ values: z.array(z.unknown()), next: z.url().optional() }).parse(await this.request(path));
      onPage?.(page.values);
      values.push(...page.values); if (values.length > 10000) throw new BitbucketUnavailable('Bitbucket pagination exceeded its item limit.');
      if (!page.next) return values;
      const next = new URL(page.next);
      if (next.origin !== this.origin || next.pathname !== endpoint || next.username || next.password || next.hash) throw new BitbucketUnavailable('Bitbucket pagination left its configured endpoint.');
      path = next.pathname.slice(this.prefix.length) + next.search;
    }
  }
  private async parsePullRequest(value: unknown) {
    const parsed = prSchema.safeParse(value);
    if (!parsed.success) throw new BitbucketUnavailable('Bitbucket returned an unexpected pull request.');
    const pr = parsed.data;
    if (pr.source.repository.uuid !== this.repository.uuid || pr.destination.repository.uuid !== this.repository.uuid
      || pr.links.html.href !== `https://bitbucket.org/${this.repository.workspace}/${this.repository.slug}/pull-requests/${pr.id}`) throw new BitbucketUnavailable('Bitbucket returned a different repository or PR identity.');
    pr.source.commit.hash = await this.fullHash(pr.source.commit.hash); pr.destination.commit.hash = await this.fullHash(pr.destination.commit.hash);
    if (pr.merge_commit) pr.merge_commit.hash = await this.fullHash(pr.merge_commit.hash);
    return pr;
  }
  private async fullHash(value: string) {
    if (hash.safeParse(value).success) return value;
    const commit = z.object({ hash }).safeParse(await this.request(`/commit/${value}`));
    if (!commit.success || !commit.data.hash.startsWith(value)) throw new BitbucketUnavailable('Bitbucket could not resolve a pull request commit.');
    return commit.data.hash;
  }
  async verifyRepository() {
    const repository = repositoryIdentity.extend({ full_name: z.string() }).parse(await this.request(''));
    if (repository.uuid !== this.repository.uuid || repository.full_name !== `${this.repository.workspace}/${this.repository.slug}`) throw new BitbucketUnavailable('Bitbucket repository identity changed. Publication is blocked.');
    return repository;
  }
  async pullRequests(branch: string, target: string) {
    // Bitbucket rejects pull-request searches above 50 per page with HTTP 400 ("Invalid pagelen").
    const query = new URLSearchParams({ q: `source.branch.name = ${JSON.stringify(branch)} AND destination.branch.name = ${JSON.stringify(target)}`, pagelen: '50' });
    for (const state of ['OPEN', 'MERGED', 'DECLINED', 'SUPERSEDED']) query.append('state', state);
    const prs = await Promise.all((await this.pages('/pullrequests?' + query)).map(item => this.parsePullRequest(item)));
    if (new Set(prs.map(pr => pr.id)).size !== prs.length || prs.some(pr => pr.source.branch.name !== branch || pr.destination.branch.name !== target)) throw new BitbucketUnavailable('Bitbucket returned an inconsistent PR search.');
    return prs;
  }
  async pullRequest(id: number) {
    z.number().int().positive().parse(id); const pr = await this.parsePullRequest(await this.request(`/pullrequests/${id}`));
    if (pr.id !== id) throw new BitbucketUnavailable('Bitbucket returned a different PR identity.'); return pr;
  }
  /** Comment pagination is independent of PR/check observation; partial pages never masquerade as an empty list. */
  async comments(id: number): Promise<BitbucketCommentRead> {
    z.number().int().positive().parse(id);
    const comments: BitbucketComment[] = []; const identities = new Set<number>();
    try {
      await this.pages(`/pullrequests/${id}/comments?pagelen=100`, values => {
        for (const value of values) {
          const parsed = commentSchema.safeParse(value);
          if (!parsed.success || identities.has(parsed.data.id) || parsed.data.pullrequest && parsed.data.pullrequest.id !== id)
            throw new BitbucketUnavailable('Bitbucket returned inconsistent PR comments.');
          identities.add(parsed.data.id); comments.push(parsed.data);
        }
      });
      return { state: 'complete', comments };
    } catch (error) {
      return { state: comments.length ? 'partial' : 'unavailable', comments,
        reason: error instanceof Error ? error.message : 'PR comments are unavailable.' };
    }
  }
  async observe(id: number): Promise<BitbucketObservation> {
    const pr = await this.pullRequest(id); const head = pr.source.commit.hash;
    const statuses = (await this.pages(`/commit/${head}/statuses?pagelen=100`)).map(item => statusSchema.parse(item));
    if (new Set(statuses.map(item => item.key)).size !== statuses.length) throw new BitbucketUnavailable('Bitbucket repeated a commit status identity.');
    // Bitbucket's pipeline list ignores nothing it does not understand: a `target.selector.type` filter returns no rows. Filter locally.
    const query = new URLSearchParams({ 'target.commit.hash': head, pagelen: '100', sort: '-created_on' });
    const rows = (await this.pages('/pipelines/?' + query)).map(item => { const parsed = pipelineSchema.safeParse(item); if (!parsed.success) throw new BitbucketUnavailable('Bitbucket returned an unexpected pipeline.'); return parsed.data; });
    const matching = rows.filter(item => item.target.type === 'pipeline_pullrequest_target' && item.target.commit.hash === head && item.target.pullrequest?.id === id
      && item.target.source === pr.source.branch.name && item.target.destination === pr.destination.branch.name && item.target.selector.type === 'pull-requests')
      .sort((a, b) => b.build_number - a.build_number || (b.run_number ?? 0) - (a.run_number ?? 0) || Date.parse(b.created_on) - Date.parse(a.created_on));
    const latest = matching[0]; const pipelines: BitbucketObservation['pipelines'] = [];
    if (latest) {
      // PR pipeline targets report a 12-character destination commit.
      if (latest.target.destination_commit) latest.target.destination_commit.hash = await this.fullHash(latest.target.destination_commit.hash);
      const steps = (await this.pages(`/pipelines/${encodeURIComponent(latest.uuid)}/steps/?pagelen=100`)).map(item => stepSchema.parse(item));
      if (new Set(steps.map(item => item.uuid)).size !== steps.length) throw new BitbucketUnavailable('Bitbucket repeated a pipeline step identity.');
      pipelines.push({ ...latest, steps });
    }
    const current = await this.pullRequest(id);
    if (JSON.stringify(current) !== JSON.stringify(pr)) throw new BitbucketUnavailable('PR changed while hosted evidence was being read. Refresh before assessing readiness.');
    return { observedAt: new Date().toISOString(), pr, statuses, pipelines };
  }
  async createPullRequest(input: { title: string; description: string; branch: string; target: string; head: string }, beforeDispatch?: () => void | Promise<void>) {
    hash.parse(input.head);
    const response = await this.request('/pullrequests', 'POST', { title: input.title, description: input.description,
      source: { branch: { name: input.branch }, commit: { hash: input.head } }, destination: { branch: { name: input.target } }, close_source_branch: false }, beforeDispatch);
    return this.sent(() => this.parsePullRequest(response));
  }
  /** Replace title and description; repeating the same request is harmless. */
  async updatePullRequest(id: number, input: { title: string; description: string }, beforeDispatch?: () => void | Promise<void>) {
    z.number().int().positive().parse(id);
    const response = await this.request(`/pullrequests/${id}`, 'PUT', { title: input.title, description: input.description }, beforeDispatch);
    const pr = await this.sent(() => this.parsePullRequest(response));
    if (pr.id !== id) throw new BitbucketUnavailable('Bitbucket returned a different PR identity.', undefined, true); return pr;
  }
  /** A mutation already reached Bitbucket: a failure reading its response, or a follow-up lookup's status, never makes it look unsent or rejected. */
  private async sent<T>(read: () => Promise<T>) {
    try { return await read(); }
    catch (error) { throw new BitbucketUnavailable(error instanceof Error ? error.message : 'Bitbucket returned an unreadable response.', undefined, true); }
  }
}
