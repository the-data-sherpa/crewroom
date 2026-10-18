import { existsSync, readFileSync } from 'node:fs';
import { isAbsolute } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { z } from 'zod';
import { bitbucketRepositorySchema, BitbucketUnavailable, type BitbucketCredential } from './bitbucket-client';
import type { PublicationConnection } from './publication';
const exec = promisify(execFile);
const metadata = z.object({ repositoryPath: z.string().refine(isAbsolute), repository: bitbucketRepositorySchema,
  policy: z.object({ statusKeys: z.array(z.string()).min(1), pipelineSteps: z.array(z.string()).min(1), optionalSteps: z.array(z.string()).default([]) }).strict() }).strict();

type Command = (file: string, args: string[], options: { timeout: number; maxBuffer: number }) => Promise<{ stdout: string }>;
const accountStatus = z.object({ active_context: z.literal('conductor'),
  contexts: z.array(z.object({ name: z.string(), host: z.string() })),
  hosts: z.array(z.object({ key: z.string(), kind: z.string(), base_url: z.string(), username: z.string().optional(), auth_method: z.string(), token_source: z.string() })) });

/** Metadata contains no tokens. Credentials and auth scheme stay inside the server. */
export function loadPublicationConnections(path: string, command: Command = exec): PublicationConnection[] {
  if (!existsSync(path)) return [];
  const entries = z.array(metadata).max(20).parse(JSON.parse(readFileSync(path, 'utf8')));
  let cached: { credential: BitbucketCredential; expiresAt: number } | undefined; let reading: Promise<BitbucketCredential> | undefined; let keychainUnavailable = false;
  async function credentials() {
    if (process.env.CREWROOM_BITBUCKET_TOKEN) return process.env.CREWROOM_BITBUCKET_TOKEN;
    if (cached && cached.expiresAt > Date.now() + 30000) return cached.credential;
    if (keychainUnavailable) throw new BitbucketUnavailable('Bitbucket credentials are unavailable to the Crewroom server. Configure server access before publication.');
    if (!reading) reading = (async () => {
      try {
        const options = { timeout: 30000, maxBuffer: 1024 * 1024 };
        const status = accountStatus.parse(JSON.parse((await command('/opt/homebrew/bin/bkt', ['--context', 'conductor', 'auth', 'status', '--json'], options)).stdout));
        const context = status.contexts.find(item => item.name === 'conductor');
        const account = status.hosts.find(item => item.key === context?.host);
        if (!account || account.key !== 'api.bitbucket.org' || account.kind !== 'cloud' || account.base_url !== 'https://api.bitbucket.org/2.0' || account.token_source !== 'keyring'
          || !['basic', 'oauth'].includes(account.auth_method)) throw new Error('Unsupported credential account');
        // The trusted CLI may refresh its OAuth account through a read-only request.
        await command('/opt/homebrew/bin/bkt', ['--context', 'conductor', 'api', '/user', '--json'], options);
        const { stdout } = await command('/usr/bin/security', ['find-generic-password', '-s', 'bkt', '-a', 'host/api.bitbucket.org/token', '-w'], { ...options, timeout: 10000 });
        if (account.auth_method === 'basic') {
          const username = z.string().min(1).regex(/^[^:\r\n]+$/).parse(account.username);
          const token = z.string().min(1).regex(/^[^\r\n]+$/).parse(stdout.replace(/\r?\n$/, ''));
          cached = { credential: { scheme: 'basic', username, token }, expiresAt: Date.now() + 300000 };
        } else {
          const token = z.object({ access_token: z.string().min(1), expires_at: z.string() }).parse(JSON.parse(stdout));
          const expiresAt = Date.parse(token.expires_at);
          if (expiresAt <= Date.now() + 30000 || !Number.isFinite(expiresAt)) throw new Error('Expired credential');
          cached = { credential: token.access_token, expiresAt };
        }
        return cached.credential;
      } catch { keychainUnavailable = true; throw new BitbucketUnavailable('Bitbucket credentials are unavailable to the Crewroom server. Configure server access before publication.'); }
    })();
    try { return await reading; } finally { reading = undefined; }
  }
  return entries.map(entry => ({ ...entry, credentials }));
}
