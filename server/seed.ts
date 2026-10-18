import type { Task } from '../shared/types';

const examples = [
  ['DEMO-101', 'Bring Jira work into one room', 'Backlog', 'High', 'Import tickets with their original status and order. Keep local execution priorities separate from Jira rank.'],
  ['DEMO-102', 'Give every ticket a clear brief', 'Backlog', 'Medium', 'Capture the outcome, open questions, and acceptance criteria before implementation begins.'],
  ['DEMO-103', 'Recover an interrupted agent session', 'In progress', 'High', 'Resume from recorded assignments and handoffs without repeating completed work.'],
  ['DEMO-104', 'Keep review evidence tied to the change', 'In review', 'High', 'Invalidate stale approvals when the proposed revision changes. Show exactly what was checked.'],
  ['DEMO-105', 'Make project guardrails visible', 'Backlog', 'Medium', 'Show which project documents and instructions were included in each assignment.'],
  ['DEMO-106', 'Add a human handoff at the PR', 'In review', 'Medium', 'Collect the change summary, review results, tests, and remaining concerns for the human reviewer.'],
];

export function sampleTasks(): Task[] {
  return examples.map(([key, title, jiraStatus, priority, description], rank) => ({
    id: key.toLowerCase(), key, title, jiraStatus, priority, description,
    source: 'sample', project: 'Sample project', rank, revision: 1, brief: '', criteria: [],
    updatedAt: new Date().toISOString(),
  }));
}
