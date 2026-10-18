import { Workbench } from './workbench';
import { JiraClient, JiraUnavailable } from './jira-client';

export class JiraSync {
  private syncing?: Promise<void>;
  constructor(private store: Workbench, readonly client: JiraClient) {}
  snapshot() { const saved = this.store.jiraState(); return saved && JSON.stringify(saved.source) === JSON.stringify(this.client.source) ? saved : { source: this.client.source, state: 'not_synced' as const, taskIds: [] }; }
  async close() { await this.syncing; }
  async refresh() {
    if (!this.syncing) this.syncing = this.performRefresh();
    try { await this.syncing; } finally { this.syncing = undefined; }
    return this.snapshot();
  }
  private async performRefresh() {
    try {
      const issues = await this.client.search(); const ids = new Set(issues.map(issue => issue.id)); const retained = [];
      const workspace = this.store.snapshot();
      for (const run of workspace.runs.filter(run => run.desiredState !== 'cancelled')) {
        const task = workspace.tasks.find(task => task.id === run.taskId)!;
        if (task.source === 'jira' && task.sourceSite === this.client.source.siteUrl && task.sourceId && !ids.has(task.sourceId)) {
          retained.push(await this.client.issue(task.sourceId)); ids.add(task.sourceId);
        }
      }
      this.store.commitJiraSync(this.client.source, issues, retained);
    }
    catch (error) { this.store.failJiraSync(this.client.source, error instanceof JiraUnavailable ? error.message : 'Jira returned an invalid source response. No partial import was saved.'); }
  }
}
