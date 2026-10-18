import { z } from 'zod';
import { jiraIssueSchema, jiraSiteSchema } from './jira';
import type { JiraSource } from '../shared/jira';

export const jiraSourceSchema = z.object({ siteUrl: jiraSiteSchema.transform(value => new URL(value).origin), cloudId: z.uuid(),
  filterId: z.string().regex(/^\d+$/), assigneeAccountId: z.string().regex(/^[A-Za-z0-9:_-]{1,200}$/), name: z.string().trim().min(1).max(120),
  inProgressStatusId: z.string().regex(/^\d+$/).optional(), inProgressTransitionId: z.string().regex(/^\d+$/).optional(),
  reviewStatusId: z.string().regex(/^\d+$/).optional(), reviewTransitionId: z.string().regex(/^\d+$/).optional(),
  doneStatusId: z.string().regex(/^\d+$/).optional(), doneTransitionId: z.string().regex(/^\d+$/).optional() }).strict();
export interface JiraConnection { source: JiraSource; credentials: () => Promise<string>; apiUrl?: string }
export class JiraUnavailable extends Error {
  constructor(message: string, readonly status?: number, readonly requestSent = false) { super(message); }
}

/** Owns Jira's wire format and pagination. Credentials never enter workflow records. */
export class JiraClient {
  readonly source: JiraSource;
  private base: string;
  constructor(private connection: JiraConnection) {
    this.source = jiraSourceSchema.parse(connection.source);
    this.base = `https://api.atlassian.com/ex/jira/${this.source.cloudId}`;
    if (connection.apiUrl) {
      const url = new URL(connection.apiUrl);
      if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.username || url.password || url.pathname !== '/' || url.search || url.hash) throw new Error('Fixture Jira endpoints must use a loopback origin.');
      this.base = url.origin;
    }
  }
  async request(path: string, method = 'GET', body?: unknown, beforeDispatch?: () => void | Promise<void>): Promise<any> {
    let requestSent = false;
    try {
      const token = await this.connection.credentials();
      if (!token || /[\r\n]/.test(token)) throw new JiraUnavailable('Jira credentials are unavailable.');
      await beforeDispatch?.();
      requestSent = true;
      const response = await fetch(this.base + '/rest/api/3/' + path, { method, redirect: 'error', signal: AbortSignal.timeout(15000),
        headers: { Authorization: `Bearer ${token}`, Accept: 'application/json', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) }, body: body === undefined ? undefined : JSON.stringify(body) });
      if (!response.ok) { await response.body?.cancel(); throw new JiraUnavailable(`Jira returned HTTP ${response.status}. Saved work is retained.`, response.status); }
      const chunks: Buffer[] = []; let bytes = 0;
      for await (const part of response.body ?? []) { bytes += part.length; if (bytes > 4 * 1024 * 1024) throw new JiraUnavailable('Jira response exceeded its size limit.'); chunks.push(Buffer.from(part)); }
      const text = Buffer.concat(chunks).toString('utf8');
      return text ? JSON.parse(text) : undefined;
    } catch (error) {
      if (error instanceof JiraUnavailable) throw new JiraUnavailable(error.message, error.status, requestSent);
      throw new JiraUnavailable('Jira is unavailable. Check the server connection and credentials; saved work is retained.', undefined, requestSent);
    }
  }
  async issue(id: string) {
    const issue = jiraIssueSchema.parse(await this.request(`issue/${encodeURIComponent(id)}?fields=summary,description,status,priority,project,updated`));
    if (issue.id !== id) throw new JiraUnavailable('Jira returned a different issue identity.');
    return issue;
  }
  async comments(id: string) {
    const comments: { id: string; body: unknown }[] = []; let startAt = 0;
    for (;;) {
      const page = z.object({ comments: z.array(z.object({ id: z.string(), body: z.unknown() })), startAt: z.number().int(), total: z.number().int().nonnegative() })
        .parse(await this.request(`issue/${encodeURIComponent(id)}/comment?startAt=${startAt}&maxResults=100&orderBy=created`));
      if (page.startAt !== startAt || comments.length + page.comments.length > 10000) throw new JiraUnavailable('Jira comment reconciliation exceeded its bounds.');
      comments.push(...page.comments);
      if (comments.length >= page.total) return comments;
      if (!page.comments.length) throw new JiraUnavailable('Jira comment pagination did not complete.');
      startAt += page.comments.length;
    }
  }
  async addComment(id: string, body: unknown, beforeDispatch?: () => void | Promise<void>) {
    return z.object({ id: z.string().min(1) }).parse(await this.request(`issue/${encodeURIComponent(id)}/comment`, 'POST', { body }, beforeDispatch));
  }
  async transitions(id: string) {
    return z.object({ transitions: z.array(z.object({ id: z.string(), to: z.object({ id: z.string() }), fields: z.record(z.string(), z.object({ required: z.boolean().optional() })).optional() })) })
      .parse(await this.request(`issue/${encodeURIComponent(id)}/transitions?expand=transitions.fields`)).transitions;
  }
  async transition(id: string, transitionId: string, beforeDispatch?: () => void | Promise<void>) { await this.request(`issue/${encodeURIComponent(id)}/transitions`, 'POST', { transition: { id: transitionId } }, beforeDispatch); }
  async search() {
    const issues: z.infer<typeof jiraIssueSchema>[] = []; const seen = new Set<string>(); let next: string | undefined;
    for (;;) {
      const query = new URLSearchParams({ jql: `filter = ${this.source.filterId} AND assignee = ${JSON.stringify(this.source.assigneeAccountId)} ORDER BY Rank ASC`,
        fields: 'summary,description,status,priority,project,updated', maxResults: '100', ...(next ? { nextPageToken: next } : {}) });
      const page = z.object({ issues: z.array(jiraIssueSchema), isLast: z.boolean(), nextPageToken: z.string().optional() }).parse(await this.request('search/jql?' + query));
      issues.push(...page.issues);
      if (issues.length > 1000 || new Set(issues.map(issue => issue.id)).size !== issues.length) throw new JiraUnavailable('Jira search exceeded the intake limit or repeated an issue. No partial import was saved.');
      if (page.isLast) return issues;
      if (!page.nextPageToken || seen.has(page.nextPageToken) || seen.size >= 100) throw new JiraUnavailable('Jira pagination did not complete. No partial import was saved.');
      next = page.nextPageToken; seen.add(next);
    }
  }
}
