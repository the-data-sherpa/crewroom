import { request } from 'node:http';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync, realpathSync, lstatSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';

/** `readOnlyWorkspace` mounts the workspace read-only: ad-hoc commands inspect the worktree; source changes go through the writer's hash-checked edits. */
export interface CommandSpec { image: string; workspace: string; command: string; args: string[]; timeoutMs?: number; memoryMiB?: number; tmpExecutable?: boolean; readOnlyWorkspace?: boolean }
export interface ContainerEngine { socket: string }
interface CommandReceipt { spec: CommandSpec; started: boolean; completed: boolean; timedOut: boolean; exitCode: number; output: string; platform: string; containerId?: string; user?: string }
class EngineError extends Error { constructor(readonly status: number | undefined) { super(`Command engine returned HTTP ${status}.`); } }
/** `user` is the uid:gid the container runs as; intents saved before it existed keep the image's default user. */
interface Intent { root?: string; token: string; name: string; spec: CommandSpec; user?: string; id?: string; startRequested?: boolean; startConfirmed?: boolean; startedAt?: number; timedOut?: boolean; completed?: { exitCode: number }; stopRequested?: boolean; removed?: boolean; removalRequested?: boolean; receipt?: CommandReceipt }
interface CommandState { state: 'running' | 'stopped' | 'uncertain'; id?: string; exitCode?: number; reason?: string }

/** The effective uid:gid of this process, which owns every workspace it prepares. */
function invokingUser() {
  if (!process.geteuid || !process.getegid) throw new Error('Command containment requires a POSIX user identity.');
  return `${process.geteuid()}:${process.getegid()}`;
}

/** A command's process namespace is its ownership boundary; a detached descendant stays inside it. */
export class ContainerCommand {
  private starting?: Promise<void>;
  constructor(readonly root: string, private engine: ContainerEngine) {
    if (!isAbsolute(engine.socket)) throw new Error('Use a local Unix socket for command containment.');
  }
  private path() { return join(this.root, 'container.json'); }
  private read(): Intent {
    // macOS exposes /var and /tmp as system aliases; every application-owned component must be canonical.
    let canonical = resolve(this.root);
    if (process.platform === 'darwin') for (const alias of ['/var', '/tmp']) {
      if (canonical.startsWith(alias + '/')) canonical = realpathSync(alias) + canonical.slice(alias.length);
    }
    if (realpathSync(this.root) !== canonical || lstatSync(this.path()).isSymbolicLink()) throw new Error('Container ownership path contains a symlink.');
    const intent: Intent = JSON.parse(readFileSync(this.path(), 'utf8'));
    if (intent.root && intent.root !== this.root) throw new Error('Container ownership directory changed.');
    return intent;
  }
  private write(value: Intent) {
    writeFileSync(this.path() + '.tmp', JSON.stringify(value), { mode: 0o600, flush: true });
    renameSync(this.path() + '.tmp', this.path());
  }
  private update(changes: Partial<Intent>) { this.write({ ...this.read(), ...changes }); }
  private temporaryMount(spec: CommandSpec) { return `rw,nosuid,${spec.tmpExecutable ? 'exec,' : ''}size=67108864`; }
  private api(method: string, path: string, body?: unknown, raw = false): Promise<any> {
    return new Promise((resolve, reject) => {
      const req = request({ socketPath: this.engine.socket, method, path, headers: { 'Content-Type': 'application/json' }, timeout: 5000 }, response => {
        const chunks: Buffer[] = []; let size = 0;
        response.on('data', chunk => { size += chunk.length; if (size > 4 * 1024 * 1024) req.destroy(new Error('Command response exceeded its limit.')); else chunks.push(chunk); });
        response.once('end', () => {
          const data = Buffer.concat(chunks);
          if (!response.statusCode || response.statusCode >= 400) { reject(new EngineError(response.statusCode)); return; }
          try { resolve(raw ? data : data.length ? JSON.parse(data.toString()) : undefined); } catch { reject(new Error('Command engine returned invalid JSON.')); }
        });
      });
      req.once('timeout', () => req.destroy(new Error('Command engine response is uncertain.')));
      req.once('error', reject); req.end(body === undefined ? undefined : JSON.stringify(body));
    });
  }
  private async remote(intent: Intent) {
    const found = await this.api('GET', `/containers/${intent.id ?? intent.name}/json`);
    if (found.Config?.Labels?.['io.crewroom.command'] !== intent.token || (intent.id && intent.id !== found.Id)
      || found.Image !== intent.spec.image || found.HostConfig?.NetworkMode !== 'none' || found.HostConfig?.Privileged
      || found.HostConfig?.Memory !== (intent.spec.memoryMiB ?? 1024) * 1024 * 1024
      || found.HostConfig?.Tmpfs?.['/tmp'] !== this.temporaryMount(intent.spec)
      || (intent.user !== undefined && found.Config?.User !== intent.user)) throw new Error('Container identity or containment changed.');
    if (!intent.id) this.update({ id: found.Id });
    return found;
  }
  async start(spec: CommandSpec) {
    if (!/^sha256:[a-f0-9]{64}$/.test(spec.image) || !isAbsolute(spec.workspace) || !isAbsolute(spec.command)) throw new Error('Commands require a pinned local image, an absolute workspace, and an absolute executable.');
    if (spec.timeoutMs !== undefined && (!Number.isSafeInteger(spec.timeoutMs) || spec.timeoutMs < 100 || spec.timeoutMs > 3600000)) throw new Error('Invalid command timeout.');
    if (spec.memoryMiB !== undefined && (!Number.isSafeInteger(spec.memoryMiB) || spec.memoryMiB < 128 || spec.memoryMiB > 8192)) throw new Error('Command memory limit must be an integer from 128 to 8192 MiB.');
    if (spec.tmpExecutable !== undefined && typeof spec.tmpExecutable !== 'boolean') throw new Error('Invalid temporary execution policy.');
    if (existsSync(this.path())) throw new Error('Command already has a durable intent; reconcile it instead of restarting.');
    mkdirSync(this.root, { recursive: true, mode: 0o700 });
    const token = randomUUID();
    // Run as the invoking owner, as the native adapter does: with CapDrop ALL a root user has no
    // DAC override, so on Linux it could not write an operator-owned 0700 workspace.
    this.write({ root: this.root, token, name: `crewroom-${token}`, spec: { ...spec, workspace: realpathSync(spec.workspace) }, user: invokingUser() });
    this.starting = this.launch();
    try { await this.starting; } finally { this.starting = undefined; }
  }
  private async launch() {
    const intent = this.read();
    const created = await this.api('POST', `/containers/create?name=${intent.name}`, {
      Image: intent.spec.image, Entrypoint: [], Cmd: [intent.spec.command, ...intent.spec.args], WorkingDir: '/workspace',
      Env: ['HOME=/tmp'], Labels: { 'io.crewroom.command': intent.token }, NetworkDisabled: true, ...(intent.user === undefined ? {} : { User: intent.user }),
      HostConfig: { NetworkMode: 'none', CapDrop: ['ALL'], SecurityOpt: ['no-new-privileges:true'], ReadonlyRootfs: true,
        PidsLimit: 256, Memory: (intent.spec.memoryMiB ?? 1024) * 1024 * 1024, NanoCpus: 2_000_000_000, RestartPolicy: { Name: 'no' },
        Mounts: [{ Type: 'bind', Source: intent.spec.workspace, Target: '/workspace', ReadOnly: intent.spec.readOnlyWorkspace === true }], Tmpfs: { '/tmp': this.temporaryMount(intent.spec) } },
    });
    this.update({ id: created.Id });
    if (this.read().stopRequested) return;
    this.update({ startRequested: true, startedAt: Date.now() });
    await this.api('POST', `/containers/${created.Id}/start`);
    this.update({ startConfirmed: true });
  }
  async inspect(): Promise<CommandState> {
    if (!existsSync(this.path())) return { state: 'stopped' };
    try {
      const intent = this.read();
      if (intent.removed) return { state: 'stopped', id: intent.id, exitCode: intent.receipt?.exitCode };
      if (intent.removalRequested && intent.receipt) {
        try { await this.remote(intent); } catch (error) {
          if (!(error instanceof EngineError) || error.status !== 404) throw error;
          this.update({ removed: true });
          return { state: 'stopped', id: intent.id, exitCode: intent.receipt.exitCode };
        }
      }
      const found = await this.remote(intent);
      if (found.State.Running || found.State.Restarting) return { state: 'running', id: found.Id };
      if (intent.startRequested && !intent.startConfirmed && found.State.Status === 'created') return { state: 'uncertain', id: found.Id, reason: 'Command start outcome is unresolved.' };
      if (found.State.Pid !== 0 || !['created', 'exited', 'dead'].includes(found.State.Status)) return { state: 'uncertain', id: found.Id, reason: 'Container termination is unconfirmed.' };
      return { state: 'stopped', id: found.Id, exitCode: found.State.ExitCode };
    } catch { return { state: 'uncertain', reason: 'Command engine ownership cannot be confirmed.' }; }
  }
  async stop(): Promise<CommandState> {
    if (!existsSync(this.path())) return { state: 'stopped' };
    try { if (this.read().removed) return this.inspect(); } catch { return { state: 'uncertain', reason: 'Container ownership directory cannot be confirmed.' }; }
    if (!this.starting) {
      const observed = await this.inspect();
      const intent = this.read();
      if (observed.state === 'stopped' && intent.startConfirmed && !intent.stopRequested && observed.exitCode !== undefined) {
        this.update({ completed: { exitCode: observed.exitCode } }); return observed;
      }
    }
    this.update({ stopRequested: true });
    try { await this.starting; } catch {}
    const before = await this.inspect();
    if (before.state !== 'running') return before;
    try { await this.api('POST', `/containers/${before.id}/kill?signal=SIGKILL`); } catch {}
    return this.inspect();
  }
  async wait(): Promise<CommandState> {
    for (;;) {
      const state = await this.inspect();
      if (state.state !== 'running') {
        const intent = this.read();
        if (state.state === 'stopped' && intent.startConfirmed && !intent.stopRequested && state.exitCode !== undefined) this.update({ completed: { exitCode: state.exitCode } });
        return state;
      }
      const intent = this.read();
      if (intent.spec.timeoutMs && intent.startedAt && Date.now() >= intent.startedAt + intent.spec.timeoutMs) {
        this.update({ timedOut: true }); return this.stop();
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
  async receipt(): Promise<CommandReceipt> {
    const saved = this.read();
    if (saved.receipt) return saved.receipt;
    const state = await this.inspect();
    if (state.state !== 'stopped' || state.exitCode === undefined) throw new Error('Command completion remains unconfirmed.');
    let intent = this.read();
    if (!intent.completed && intent.startConfirmed && !intent.stopRequested && !intent.timedOut) { this.update({ completed: { exitCode: state.exitCode } }); intent = this.read(); }
    const image = await this.api('GET', `/images/${intent.spec.image}/json`);
    const receipt = { spec: intent.spec, started: intent.startConfirmed === true, completed: !!intent.completed && intent.completed.exitCode === state.exitCode && !intent.timedOut,
      timedOut: intent.timedOut === true, exitCode: state.exitCode, output: await this.logs(), platform: `${image.Os}/${image.Architecture}`, containerId: state.id,
      ...(intent.user === undefined ? {} : { user: intent.user }) };
    this.update({ receipt });
    return receipt;
  }

  async logs() {
    const intent = this.read();
    if (intent.receipt) return intent.receipt.output;
    await this.remote(intent);
    const data: Buffer = await this.api('GET', `/containers/${intent.id ?? intent.name}/logs?stdout=true&stderr=true`, undefined, true);
    const parts: Buffer[] = [];
    for (let offset = 0; offset + 8 <= data.length;) {
      const length = data.readUInt32BE(offset + 4); offset += 8;
      if (offset + length > data.length) throw new Error('Incomplete command output.');
      parts.push(data.subarray(offset, offset + length)); offset += length;
    }
    return Buffer.concat(parts).toString();
  }
  /** Streams output while the container runs. Observation only: errors end the stream, never the command. */
  async follow(onChunk: (chunk: Buffer) => void, signal: AbortSignal) {
    const intent = this.read(); await this.remote(intent);
    await new Promise<void>(resolve => {
      const req = request({ socketPath: this.engine.socket, method: 'GET', path: `/containers/${intent.id ?? intent.name}/logs?stdout=true&stderr=true&follow=true`, signal }, response => {
        if (!response.statusCode || response.statusCode >= 400) { response.resume(); resolve(); return; }
        let pending = Buffer.alloc(0);
        response.on('data', (data: Buffer) => {
          pending = Buffer.concat([pending, data]);
          while (pending.length >= 8) {
            const length = pending.readUInt32BE(4);
            if (pending.length < 8 + length) break;
            onChunk(pending.subarray(8, 8 + length)); pending = pending.subarray(8 + length);
          }
        });
        response.once('end', () => resolve()); response.once('error', () => resolve());
      });
      req.once('error', () => resolve()); req.end();
    });
  }
  async remove() {
    const state = await this.inspect();
    if (state.state !== 'stopped' || !state.id || this.read().removed) return;
    // Persist final logs and execution identity before deleting the engine's only copy.
    await this.receipt();
    this.update({ removalRequested: true });
    try { await this.api('DELETE', `/containers/${state.id}`); } catch (error) {
      if (!(error instanceof EngineError) || error.status !== 404) throw error;
    }
    this.update({ removed: true });
  }
}
