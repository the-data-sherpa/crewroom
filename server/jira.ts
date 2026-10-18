import { z } from 'zod';

const textNode: z.ZodType<{ text?: string; type?: string; content?: unknown[] }> = z.object({
  text: z.string().optional(), type: z.string().optional(), content: z.array(z.unknown()).optional(),
});

export function descriptionText(value: unknown, depth = 0): string {
  if (typeof value === 'string') return value;
  if (value == null || depth > 30) return '';
  const parsed = textNode.safeParse(value);
  if (!parsed.success) return '';
  const node = parsed.data;
  if (node.type === 'hardBreak') return '\n';
  const text = node.text ?? (node.content ?? []).map(item => descriptionText(item, depth + 1)).join('');
  return ['paragraph', 'heading', 'listItem'].includes(node.type ?? '') ? `${text}\n` : text;
}

export const jiraSiteSchema = z.url().refine(value => {
    const url = new URL(value);
    return url.protocol === 'https:' && !url.username && !url.password && !url.search && !url.hash && url.pathname === '/';
  }, 'Use your Jira HTTPS site address, without a path or credentials.');
export const jiraIssueSchema = z.object({
    id: z.string().min(1).max(200), key: z.string().regex(/^[A-Za-z][A-Za-z0-9_]*-\d+$/),
    fields: z.object({
      summary: z.string().trim().min(1).max(300), description: z.unknown().optional(),
      status: z.object({ id: z.string().optional(), name: z.string().trim().min(1).max(80) }),
      updated: z.string().refine(value => Number.isFinite(Date.parse(value)), 'Invalid source timestamp.').optional(),
      priority: z.object({ name: z.string().max(80) }).nullish(),
      project: z.object({ name: z.string().max(100) }).optional(),
    }),
  });
export const jiraImportSchema = z.object({ siteUrl: jiraSiteSchema,
  issues: z.array(jiraIssueSchema).min(1).max(1000),
}).refine(value => new Set(value.issues.map(issue => issue.id)).size === value.issues.length, 'The import contains duplicate issue IDs.');
