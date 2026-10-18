import { existsSync, mkdirSync, readdirSync, writeFileSync, realpathSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { homedir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { launchOwned, inspectOwned, stopOwned } from './runtime/processes';

const hash = z.string().regex(/^[a-f0-9]{40,64}$/);
const branch = z.string().max(200).regex(/^[A-Za-z0-9][A-Za-z0-9._/-]*$/).refine(value => !value.includes('..') && !value.includes('//') && !value.endsWith('/') && !value.endsWith('.') && !value.split('/').some(part => part.endsWith('.lock') || part.startsWith('.')));

/** Backend-owned Git transport with explicit remote/ref admission and process ownership. */
export class PublicationGit {
  private remote: string;
  private closing = false;
  private recovering?: Promise<void>;
  private jobs = new Map<string, Promise<unknown>>();
  constructor(private root: string, remote: { workspace: string; slug: string } | { fixturePath: string }, private protectedBranches = ['main', 'master']) {
    if ('fixturePath' in remote) {
      if (!isAbsolute(remote.fixturePath)) throw new Error('A fixture remote requires an absolute path.'); this.remote = realpathSync(remote.fixturePath);
    } else {
      z.string().regex(/^[A-Za-z0-9_-]+$/).parse(remote.workspace); z.string().regex(/^[A-Za-z0-9_.-]+$/).parse(remote.slug);
      this.remote = `git@bitbucket.org:${remote.workspace}/${remote.slug}.git`;
    }
    mkdirSync(root, { recursive: true, mode: 0o700 });
  }
  private async recover() {
    if (!this.recovering) this.recovering = (async () => {
      for (const entry of readdirSync(this.root, { withFileTypes: true }).filter(entry => entry.isDirectory())) {
        const root = join(this.root, entry.name); if (existsSync(join(root, 'stopped.json'))) continue;
        const ownership = await stopOwned(root);
        if (ownership.state !== 'stopped') throw new Error('Publication process ownership is unresolved. No replacement command was started.');
        writeFileSync(join(root, 'stopped.json'), JSON.stringify(ownership), { mode: 0o600, flush: true });
      }
    })();
    await this.recovering;
  }
  private async run(args: string[], cwd = this.root) {
    await this.recover(); if (this.closing) throw new Error('Publication transport is stopping.');
    const root = join(this.root, randomUUID());
    const env = { PATH: process.env.PATH, HOME: homedir(), LANG: 'C', SSH_AUTH_SOCK: process.env.SSH_AUTH_SOCK, GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', GIT_SSH_COMMAND: '/usr/bin/ssh -o BatchMode=yes -o StrictHostKeyChecking=yes' };
    const child = launchOwned(root, '/usr/bin/git', ['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'credential.helper=', ...args], { cwd, env });
    let output = '', bytes = 0, timedOut = false;
    const consume = (chunk: Buffer, keep: boolean) => { bytes += chunk.length; if (bytes <= 128 * 1024 && keep) output += chunk.toString(); if (bytes > 128 * 1024) void stopOwned(root); };
    child.stdout.on('data', chunk => consume(chunk, true)); child.stderr.on('data', chunk => consume(chunk, false)); child.stdin.end();
    const pending = (async () => {
      let timer: ReturnType<typeof setTimeout>;
      const timeout = new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => {
          timedOut = true;
          void stopOwned(root).finally(() => {
            child.stdout.destroy(); child.stderr.destroy(); child.unref();
            reject(new Error('Publication command timed out. Reconcile process ownership and the remote outcome.'));
          }).catch(() => {});
        }, 30000);
      });
      try {
        const code = await Promise.race([new Promise<number | null>((resolve, reject) => { child.once('error', () => reject(new Error('Publication command could not start.'))); child.once('close', resolve); }), timeout]);
        const ownership = await inspectOwned(root);
        if (ownership.state !== 'stopped') throw new Error('Publication process ownership is unresolved.');
        writeFileSync(join(root, 'stopped.json'), JSON.stringify(ownership), { mode: 0o600, flush: true });
        if (timedOut || bytes > 128 * 1024) throw new Error('Publication command exceeded its limit. Reconcile the remote outcome.');
        return { code, output };
      } finally { clearTimeout(timer!); }
    })();
    this.jobs.set(root, pending);
    try { return await pending; } finally { this.jobs.delete(root); }
  }
  async ref(name: string) {
    branch.parse(name); const result = await this.run(['ls-remote', '--exit-code', '--refs', this.remote, `refs/heads/${name}`]);
    if (result.code === 2 && !result.output.trim()) return undefined;
    if (result.code !== 0) throw new Error('Remote branch observation is unavailable.');
    const lines = result.output.trim().split('\n'); const [head, ref] = lines[0].split(/\s+/);
    if (lines.length !== 1 || ref !== `refs/heads/${name}` || !hash.safeParse(head).success) throw new Error('Remote branch observation is inconsistent.'); return head;
  }
  async push(input: { directory: string; head: string; branch: string; expectedHead: string | null }) {
    branch.parse(input.branch); hash.parse(input.head); if (input.expectedHead !== null) hash.parse(input.expectedHead);
    if (!isAbsolute(input.directory)) throw new Error('Publication requires an absolute owned checkout.');
    if (this.protectedBranches.includes(input.branch)) throw new Error('Publication cannot push a protected target branch.');
    const result = await this.run(['push', '--porcelain', `--force-with-lease=refs/heads/${input.branch}:${input.expectedHead ?? ''}`, this.remote, `${input.head}:refs/heads/${input.branch}`], input.directory);
    if (result.code !== 0) throw new Error('Git publication was not confirmed. Reconcile the remote branch before another write.');
  }
  /** Fetch the remote target into a Crewroom-owned reference of the owned repository; operator refs are untouched. */
  async fetchTarget(input: { directory: string; branch: string; destination: string }) {
    branch.parse(input.branch);
    const destination = /^refs\/crewroom\/([^/]+)\/refresh\/([^/]+)\/target$/.exec(input.destination);
    if (!destination) throw new Error('A target fetch requires a request-owned snapshot ref.');
    z.string().uuid().parse(destination[1]); z.string().uuid().parse(destination[2]);
    if (!isAbsolute(input.directory)) throw new Error('Target refresh requires an absolute owned repository.');
    const result = await this.run(['fetch', '--no-tags', '--no-write-fetch-head', '--no-recurse-submodules', '--quiet', this.remote, `+refs/heads/${input.branch}:${input.destination}`], input.directory);
    if (result.code !== 0) throw new Error('The remote target could not be fetched.');
  }
  async close() {
    this.closing = true;
    const jobs = [...this.jobs];
    for (const [root] of jobs) await stopOwned(root);
    await Promise.allSettled(jobs.map(([, promise]) => promise));
  }
}
