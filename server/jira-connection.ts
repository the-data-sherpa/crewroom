import { readFileSync, existsSync } from 'node:fs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { gunzipSync } from 'node:zlib';
import { z } from 'zod';
import { jiraSourceSchema, JiraUnavailable, type JiraConnection } from './jira-client';

const exec = promisify(execFile);
const accountSchema = jiraSourceSchema.extend({ accountId: z.string().regex(/^[A-Za-z0-9:_-]{1,200}$/) }).strict();
/** ACLI owns login and refresh. Only the loopback server reads its existing Keychain credential. */
export function loadJiraConnection(path: string): JiraConnection | undefined {
  if (!existsSync(path)) return undefined;
  const { accountId, ...source } = accountSchema.parse(JSON.parse(readFileSync(path, 'utf8')));
  let renewing: Promise<string> | undefined;
  async function readToken() {
    const { stdout } = await exec('/usr/bin/security', ['find-generic-password', '-s', 'acli', '-a', `oauth:${source.cloudId}:${accountId}`, '-w'], { timeout: 10000, maxBuffer: 1024 * 1024 });
    const value = stdout.trim(); const split = value.indexOf(':'); const format = value.slice(0, split);
    if (!['go-keyring-base64', 'go-keyring-encoded'].includes(format)) throw new Error('Unsupported keychain format');
    const record = JSON.parse(gunzipSync(Buffer.from(value.slice(split + 1), format === 'go-keyring-base64' ? 'base64' : 'hex'), { maxOutputLength: 1024 * 1024 }).toString());
    return typeof record.access_token === 'string' && Date.parse(record.expiry) > Date.now() + 30000 ? record.access_token as string : undefined;
  }
  async function credentials() {
    try {
      const token = await readToken(); if (token) return token;
      if (!renewing) renewing = (async () => {
        await exec('/opt/homebrew/bin/acli', ['jira', 'workitem', 'search', '--filter', source.filterId, '--limit', '1', '--json'], { timeout: 30000, maxBuffer: 2 * 1024 * 1024 });
        const refreshed = await readToken(); if (!refreshed) throw new Error('Credential not renewed'); return refreshed;
      })();
      try { return await renewing; } finally { renewing = undefined; }
    } catch { throw new JiraUnavailable('Jira credentials are unavailable. Sign in to the configured account with ACLI on this machine.'); }
  }
  return { source, credentials };
}
