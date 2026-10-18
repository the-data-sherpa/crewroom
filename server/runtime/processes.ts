import { execFile, spawn, type ChildProcess, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, existsSync, fsyncSync, mkdirSync, openSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { promisify } from 'node:util';

interface ProcessIdentity { pid: number; pgid: number; started: string; token: string }
export interface Ownership { state: 'live' | 'stopped' | 'uncertain'; reason?: string; owner?: ProcessIdentity; members?: number[] }
const helper = resolve('server/runtime/process-lease.py');
const execute = promisify(execFile);

/** `lost` resolves on holder exit, including expected close. The helper also fences the persisted epoch under its dispatch lock. */
export interface ControllerAuthority {
  root: string; epoch: string; lost: Promise<void>;
  assertCurrent(): void;
  close(): Promise<void>;
}
export async function acquireController(root: string): Promise<ControllerAuthority> {
  mkdirSync(root, { recursive: true, mode: 0o700 });
  const deadline = Date.now() + 1200;
  for (;;) {
    const epoch = randomUUID();
    const holder = spawn('python3', [helper, 'hold', root, epoch], { stdio: 'pipe' });
    holder.stderr.resume();
    let active = true;
    const lost = new Promise<void>(resolve => holder.once('exit', () => { active = false; resolve(); }));
    // The configured TypeScript library predates Promise.withResolvers.
    const status = await new Promise<'held' | 'controller-held' | 'dispatch-busy'>((resolve, reject) => {
      let response = '';
      holder.stdout.on('data', data => {
        response += String(data);
        if (response.length > 64) { reject(new Error('Controller lock response was invalid.')); return; }
        const newline = response.indexOf('\n');
        if (newline < 0) return;
        const line = response.slice(0, newline);
        if (line === 'held' || line === 'controller-held' || line === 'dispatch-busy') resolve(line);
        else reject(new Error('Controller lock response was invalid.'));
      });
      holder.once('error', () => reject(new Error('Unable to acquire the workbench controller lock.')));
      holder.once('close', () => reject(new Error('Controller lock failed.')));
    });
    if (status === 'held') {
      return { root: realpathSync(root), epoch, lost,
        assertCurrent() {
          if (!active || holder.exitCode !== null || holder.signalCode !== null ||
              readFileSync(join(root, 'epoch.json'), 'utf8') !== JSON.stringify({ epoch })) throw new Error('Controller authority lost.');
        },
        async close() { active = false; holder.stdin.end(); await lost; },
      };
    }
    await lost;
    if (status === 'controller-held') throw new Error('Another controller owns this workbench.');
    if (Date.now() >= deadline) throw new Error('Controller dispatch lock remains held while controller lease is free; inspect checked dispatch helper and retry.');
    await new Promise(resolve => setTimeout(resolve, 30));
  }
}

/** Persist this exact descriptor with the caller's intent; never reconstruct an old token or replace its root. */
export interface CheckedProcess { version: 1; root: string; token: string; specSha256: string; epoch: string; controllerRoot: string }
type CommandOptions = { cwd: string; env: NodeJS.ProcessEnv };
const admittedEnv: Record<string, true> = { PATH: true, HOME: true, LANG: true, SSH_AUTH_SOCK: true, GIT_TERMINAL_PROMPT: true, GIT_CONFIG_GLOBAL: true, GIT_CONFIG_NOSYSTEM: true, GIT_SSH_COMMAND: true };
function canonical(path: string) {
  if (!isAbsolute(path) || resolve(path) !== path || realpathSync(path) !== path) throw new Error('Process directory is not canonical.');
  return path;
}
function checkedManifest(descriptor: CheckedProcess) {
  canonical(descriptor.root);
  const raw = readFileSync(join(descriptor.root, 'launch.json'));
  const config = JSON.parse(raw.toString('utf8'));
  if (descriptor.version !== 1 || config.version !== 1 || config.token !== descriptor.token ||
      config.epoch !== descriptor.epoch || config.controllerRoot !== descriptor.controllerRoot ||
      createHash('sha256').update(raw).digest('hex') !== descriptor.specSha256) throw new Error('Checked process manifest does not match its descriptor.');
  return config;
}

/** Pure checked identity: compute the exact immutable bytes before the caller persists its intent. */
function checkedSpec(root: string, token: string, command: string, args: string[], options: CommandOptions, epoch: string, controllerRoot: string) {
  if (!/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(token) || !isAbsolute(command) ||
      !Array.isArray(args) || args.some(arg => typeof arg !== 'string') || !isAbsolute(options.cwd)) throw new Error('Invalid checked command identity.');
  canonical(options.cwd); canonical(controllerRoot); canonical(dirname(root));
  const env: Record<string, string> = {};
  for (const key of Object.keys(options.env).sort()) {
    const value = options.env[key];
    if (!admittedEnv[key] || typeof value !== 'string') throw new Error('Checked command environment is not allowlisted.');
    env[key] = value;
  }
  const raw = JSON.stringify({ version: 1, token, command, args, cwd: options.cwd, env, epoch, controllerRoot });
  const descriptor: CheckedProcess = { version: 1, root, token, specSha256: createHash('sha256').update(raw).digest('hex'), epoch, controllerRoot };
  return { descriptor, raw };
}
export function describeCheckedSpec(root: string, token: string, command: string, args: string[], options: CommandOptions, epoch: string, controllerRoot: string): CheckedProcess {
  return checkedSpec(root, token, command, args, options, epoch, controllerRoot).descriptor;
}
export function describeCheckedOwned(root: string, token: string, command: string, args: string[], options: CommandOptions, authority: ControllerAuthority): CheckedProcess {
  authority.assertCurrent();
  return describeCheckedSpec(root, token, command, args, options, authority.epoch, authority.root);
}
/** Only after the descriptor is durably admitted may this create the root and launch.json. Existing manifests are never rewritten. */
export function prepareCheckedOwned(root: string, token: string, command: string, args: string[], options: CommandOptions, authority: ControllerAuthority): CheckedProcess {
  authority.assertCurrent();
  const { descriptor, raw } = checkedSpec(root, token, command, args, options, authority.epoch, authority.root);
  if (!existsSync(root)) {
    mkdirSync(root, { mode: 0o700 });
    const parent = openSync(dirname(root), 'r'); try { fsyncSync(parent); } finally { closeSync(parent); }
    writeFileSync(join(root, 'launch.json'), raw, { mode: 0o600, flag: 'wx', flush: true });
    const directory = openSync(root, 'r'); try { fsyncSync(directory); } finally { closeSync(directory); }
  }
  checkedManifest(descriptor);
  return descriptor;
}

/**
 * Dispatches only a prepared descriptor under its original live controller epoch. The
 * checked helper alone bounds stdout.log/stderr.log to 128 KiB per stream; its separate
 * stderr pipe carries short launcher diagnostics, never admitted command output. The
 * returned ChildProcess reports a live-parent exit; no crash-durable exit receipt exists.
 */
export function launchCheckedOwned(descriptor: CheckedProcess, authority: ControllerAuthority): ChildProcess {
  authority.assertCurrent();
  checkedManifest(descriptor);
  if (descriptor.epoch !== authority.epoch || descriptor.controllerRoot !== authority.root) throw new Error('Stale controller epoch.');
  if (existsSync(join(descriptor.root, 'owner.json')) || existsSync(join(descriptor.root, 'seal.json'))) throw new Error('Checked process was already launched or sealed.');
  return spawn('python3', [helper, 'checked-exec', descriptor.root, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot],
    { cwd: descriptor.root, env: { PATH: process.env.PATH, LANG: 'en_US.UTF-8', CREWROOM_PROCESS_INSPECTOR: process.env.CREWROOM_PROCESS_INSPECTOR }, detached: true, stdio: ['ignore', 'ignore', 'pipe'] });
}

/** Verify an existing manifest, or a sealed manifest-less intent, before trusting its inherited lease. */
export async function inspectCheckedOwned(descriptor: CheckedProcess): Promise<Ownership> {
  try {
    canonical(descriptor.root);
    const unprepared = !existsSync(join(descriptor.root, 'launch.json'));
    if (unprepared) {
      if (readdirSync(descriptor.root).some(name => name !== 'lease.lock' && name !== 'seal.json')) throw new Error('Unprepared root has other files.');
    } else checkedManifest(descriptor);
    if (existsSync(join(descriptor.root, 'seal.json'))) {
      const seal = JSON.parse(readFileSync(join(descriptor.root, 'seal.json'), 'utf8'));
      if (seal.token !== descriptor.token || seal.specSha256 !== descriptor.specSha256) throw new Error('Seal mismatch.');
      if (!existsSync(join(descriptor.root, 'owner.json'))) {
        const lock = (await execute('python3', [helper, 'probe', descriptor.root], { encoding: 'utf8', timeout: 5000 })).stdout.trim();
        return lock === 'free' ? { state: 'stopped', reason: unprepared ? 'Admitted launch sealed before manifest creation.' : 'Prepared launch durably sealed before execution.' }
          : { state: 'uncertain', reason: 'Sealed lease still has a holder.' };
      }
    }
    if (unprepared) throw new Error('Manifest-less process is not sealed.');
    if (existsSync(join(descriptor.root, 'owner.json'))) {
      const owner = JSON.parse(readFileSync(join(descriptor.root, 'owner.json'), 'utf8'));
      if (owner.specSha256 !== descriptor.specSha256 || owner.epoch !== descriptor.epoch) throw new Error('Owner mismatch.');
    }
    return inspectSettled(descriptor.root);
  } catch { return { state: 'uncertain', reason: 'Checked process identity cannot be established.' }; }
}

/** Seal the operation lease even when admission preceded root/manifest creation. Never manufacture a manifest. */
export async function sealCheckedOwned(descriptor: CheckedProcess): Promise<Ownership> {
  try {
    if (descriptor.version !== 1 || !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(descriptor.token)
      || !/^[0-9a-f]{64}$/i.test(descriptor.specSha256)) throw new Error('Invalid checked descriptor.');
    canonical(descriptor.controllerRoot); canonical(dirname(descriptor.root));
    if (!existsSync(descriptor.root)) {
      try { mkdirSync(descriptor.root, { mode: 0o700 }); }
      catch (error) { if (!existsSync(descriptor.root)) throw error; }
      const parent = openSync(dirname(descriptor.root), 'r'); try { fsyncSync(parent); } finally { closeSync(parent); }
    }
    canonical(descriptor.root);
    if (existsSync(join(descriptor.root, 'launch.json'))) checkedManifest(descriptor);
    else if (readdirSync(descriptor.root).some(name => name !== 'lease.lock' && name !== 'seal.json')) throw new Error('Unprepared root has other files.');
  } catch { return { state: 'uncertain', reason: 'Checked process manifest does not match its descriptor.' }; }
  try { await execute('python3', [helper, 'seal', descriptor.root, descriptor.token, descriptor.specSha256], { encoding: 'utf8', timeout: 5000 }); }
  catch { return inspectCheckedOwned(descriptor); }
  return inspectCheckedOwned(descriptor);
}

export function prepareOwned(root: string, command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv }) {
  mkdirSync(root, { recursive: true, mode: 0o700 });
  writeFileSync(join(root, 'launch.json'), JSON.stringify({ token: randomUUID(), command, args, ...options }), { mode: 0o600, flag: 'wx', flush: true });
  return ['python3', helper, 'exec', root];
}

export function launchOwned(root: string, command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv }): ChildProcessWithoutNullStreams {
  const argv = prepareOwned(root, command, args, options);
  return spawn(argv[0], argv.slice(1), { cwd: options.cwd, env: { PATH: process.env.PATH, LANG: 'en_US.UTF-8', CREWROOM_PROCESS_INSPECTOR: process.env.CREWROOM_PROCESS_INSPECTOR }, detached: true, stdio: 'pipe' });
}

async function processTable() {
  return (await execute(process.env.CREWROOM_PROCESS_INSPECTOR ?? '/bin/ps', ['-axo', 'pid=,pgid=,stat=,lstart='], { encoding: 'utf8', timeout: 5000 })).stdout.trim().split('\n').map(line => {
    const fields = line.trim().split(/\s+/);
    // Zombies (state Z) and processes already exiting (flag E) have released their descriptors and run no further code.
    return { pid: Number(fields[0]), pgid: Number(fields[1]), gone: fields[2].startsWith('Z') || fields[2].slice(1).includes('E'), started: fields.slice(3).join(' ') };
  }).filter(item => !item.gone);
}

/** `scanHolders: false` skips the slow all-process lock scan; a non-live result then lists group members only. */
export async function inspectOwned(root: string, scanHolders = true): Promise<Ownership> {
  if (!existsSync(join(root, 'launch.json'))) return { state: 'stopped' };
  try {
    const intent = JSON.parse(readFileSync(join(root, 'launch.json'), 'utf8'));
    const owner: ProcessIdentity = JSON.parse(readFileSync(join(root, 'owner.json'), 'utf8'));
    if (owner.token !== intent.token || owner.pid < 2 || owner.pid !== owner.pgid) return { state: 'uncertain', reason: 'Process identity does not match its launch intent.' };
    const processes = await processTable();
    const current = processes.find(item => item.pid === owner.pid);
    const members = processes.filter(item => item.pgid === owner.pgid).map(item => item.pid);
    const lock = (await execute('python3', [helper, 'probe', root], { encoding: 'utf8', timeout: 5000 })).stdout.trim();
    if (!current && !members.length && lock === 'free') return { state: 'stopped', owner, members };
    if (current && (current.started !== owner.started.replace(/\s+/g, ' ') || current.pgid !== owner.pgid)) return { state: 'uncertain', reason: 'PID identity changed; refusing to signal it.', owner, members };
    if (lock !== 'held') return { state: 'uncertain', reason: 'A process survives without its inherited ownership lock.', owner, members };
    // Kernel file ownership binds a currently live PID to this unique, never-unlinked lock inode.
    // The leader is checked with a scoped query; scanning every process's open files takes seconds under load.
    const holding = async (args: string[]) => { try { return (await execute('/usr/sbin/lsof', ['-t', ...args, join(root, 'lease.lock')], { encoding: 'utf8', timeout: 5000 })).stdout.trim().split('\n').filter(Boolean).map(Number); } catch { return []; } };
    if (current && (await holding(['-a', '-p', String(owner.pid)])).includes(owner.pid)) return { state: 'live', owner, members };
    // Without a live leader, find every holder, including descendants that left the process group.
    const holders = scanHolders ? await holding([]) : [];
    return { state: 'uncertain', reason: 'The leader exited; surviving lock holders require reconciliation.', owner, members: [...new Set([...members, ...holders])] };
  } catch { return { state: 'uncertain', reason: 'Launch ownership cannot be established.' }; }
}

/**
 * An exiting process briefly shows a released lock or a vanished leader; re-inspect until it settles or the window ends.
 * Rechecks skip the all-process lock scan so the window stays bounded; an unsettled result keeps the holders found first.
 */
async function settled(root: string, first: Ownership, windowMs: number) {
  let state = first; const deadline = Date.now() + windowMs;
  while (state.state === 'uncertain' && Date.now() + 50 < deadline) {
    await new Promise(resolve => setTimeout(resolve, 50));
    // A recheck that outlives the window leaves the earlier answer standing; its probes end on their own timeouts.
    let expired!: () => void; const window = new Promise<undefined>(resolve => { const timer = setTimeout(resolve, Math.max(0, deadline - Date.now())); expired = () => clearTimeout(timer); });
    const next = await Promise.race([inspectOwned(root, false), window]); expired();
    if (!next) break;
    state = next;
  }
  return state.state === 'uncertain' && state !== first ? { ...state, members: [...new Set([...(state.members ?? []), ...(first.members ?? [])])] } : state;
}
/** Point-in-time ownership, re-read briefly while a process is mid-exit. */
export async function inspectSettled(root: string, windowMs = 1000) { return settled(root, await inspectOwned(root), windowMs); }

export async function stopOwned(root: string): Promise<Ownership> {
  let state = await settled(root, await inspectOwned(root), 1000);
  if (state.state !== 'live') return state;
  // Signal a group only while its leader still holds this job's kernel lock and identity.
  try { process.kill(-state.owner!.pgid, 'SIGTERM'); } catch { return settled(root, await inspectOwned(root), 1000); }
  const deadline = Date.now() + 600;
  while (Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 30));
    state = await inspectOwned(root); if (state.state === 'stopped') return state;
  }
  if (state.state === 'live') { try { process.kill(-state.owner!.pgid, 'SIGKILL'); } catch {} }
  await new Promise(resolve => setTimeout(resolve, 50));
  return settled(root, await inspectOwned(root), 1000);
}
