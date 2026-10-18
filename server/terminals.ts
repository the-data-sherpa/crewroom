import { spawn, execFile, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { appendFileSync, existsSync, openSync, readSync, closeSync, statSync, writeFileSync, readFileSync, watch, type FSWatcher } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

/** Width and height every agent window uses, so browser tiles can render panes exactly. */
export const PANE_COLUMNS = 80;
export const PANE_ROWS = 24;
const LOG_LIMIT = 2 * 1024 * 1024;
export const REPLAY_BYTES = 96 * 1024;

export type TerminalListener = (chunk: Buffer) => void;

/** Decodes tmux control-mode %output payloads: octal escapes for control characters and backslash. */
export function decodeControlOutput(value: string): Buffer {
  const bytes: number[] = [];
  for (let index = 0; index < value.length; index++) {
    const char = value[index];
    if (char === '\\' && /^[0-7]{3}$/.test(value.slice(index + 1, index + 4))) {
      bytes.push(parseInt(value.slice(index + 1, index + 4), 8)); index += 3;
    } else bytes.push(...Buffer.from(char, 'utf8'));
  }
  return Buffer.from(bytes);
}

export function tail(path: string, bytes: number) {
  if (!existsSync(path)) return Buffer.alloc(0);
  const size = statSync(path).size; const start = Math.max(0, size - bytes);
  const buffer = Buffer.alloc(size - start); const fd = openSync(path, 'r');
  try { readSync(fd, buffer, 0, buffer.length, start); } finally { closeSync(fd); }
  return buffer;
}

/**
 * Observes Crewroom's private tmux server through one control-mode client.
 * Read-only: it never sends keys to a pane. Pane output is also written to a
 * bounded per-attempt log so exited or disconnected panes can still be replayed.
 */
export class TerminalHub {
  private client?: ChildProcessWithoutNullStreams;
  private listeners = new Map<string, Set<TerminalListener>>();
  private paneExited = new Set<(pane: string) => void>();
  private retry?: ReturnType<typeof setTimeout>;
  private closed = false;
  connected = false;

  constructor(private socket: string, private runtimeRoot: string, private paneOwner: (pane: string) => string | undefined) { this.ensureClient(); }

  private env() { return { PATH: process.env.PATH, HOME: homedir(), LANG: 'en_US.UTF-8', TMPDIR: this.runtimeRoot }; }
  private tmux(args: string[]) {
    return new Promise<string>((resolve, reject) => execFile('tmux', ['-S', this.socket, '-f', '/dev/null', ...args],
      { encoding: 'utf8', timeout: 5000, env: this.env(), maxBuffer: 8 * 1024 * 1024 }, (error, stdout) => error ? reject(error) : resolve(stdout)));
  }

  private ensureClient() {
    // Attached whenever Crewroom's tmux server exists, so the replay log is complete even with no viewer.
    if (this.client || this.closed) return;
    if (!existsSync(this.socket)) { this.scheduleRetry(); return; }
    const client = spawn('tmux', ['-S', this.socket, '-f', '/dev/null', '-C', 'attach-session', '-t', 'crewroom'], { env: this.env(), stdio: 'pipe' });
    this.client = client;
    client.stdin.on('error', () => {});
    client.stdin.write(`refresh-client -C ${PANE_COLUMNS}x${PANE_ROWS}\nrefresh-client -B 'dead:%*:#{pane_dead}'\n`);
    createInterface({ input: client.stdout }).on('line', line => {
      if (line.startsWith('%output ')) {
        const space = line.indexOf(' ', 8);
        const pane = line.slice(8, space); const data = decodeControlOutput(line.slice(space + 1));
        this.connected = true;
        this.record(pane, data);
        for (const listener of this.listeners.get(pane) ?? []) listener(data);
      } else if (line.startsWith('%subscription-changed dead ')) {
        // tmux 3.7c: %subscription-changed dead $session @window index %pane : value
        const match = /^%subscription-changed dead \$\S+ @\S+ \d+ (%\d+) : 1$/.exec(line);
        if (match) for (const listener of this.paneExited) listener(match[1]);
      } else if (line.startsWith('%begin') || line.startsWith('%session-changed')) this.connected = true;
    });
    client.once('exit', () => {
      if (this.client === client) { this.client = undefined; this.connected = false; }
      this.scheduleRetry();
    });
  }
  private scheduleRetry() {
    if (this.retry || this.closed) return;
    this.retry = setTimeout(() => { this.retry = undefined; this.ensureClient(); }, 1000);
    this.retry.unref();
  }

  logPath(attemptId: string) { return join(this.runtimeRoot, attemptId, 'terminal.log'); }
  private owners = new Map<string, { attemptId?: string; at: number }>();
  private owner(pane: string) {
    const cached = this.owners.get(pane);
    if (cached && Date.now() - cached.at < 5000) return cached.attemptId;
    const attemptId = this.paneOwner(pane); this.owners.set(pane, { attemptId, at: Date.now() }); return attemptId;
  }
  private record(pane: string, data: Buffer) {
    const attemptId = this.owner(pane); if (!attemptId || !existsSync(join(this.runtimeRoot, attemptId))) return;
    const path = this.logPath(attemptId);
    try {
      appendFileSync(path, data, { mode: 0o600 });
      if (statSync(path).size > LOG_LIMIT) writeFileSync(path, tail(path, LOG_LIMIT / 2), { mode: 0o600 });
    } catch { /* The log is a replay convenience, not evidence. */ }
  }

  /** The current screen with styles, falling back to the retained log once the pane is gone. */
  async snapshot(pane: string | undefined, attemptId: string) {
    if (pane) {
      try {
        const screen = await this.tmux(['capture-pane', '-p', '-e', '-J', '-S', '-400', '-t', pane]);
        return { source: 'pane' as const, data: Buffer.from(screen.replace(/\n$/, '').replace(/\n/g, '\r\n')) };
      } catch { /* Pane or server gone: use the retained log. */ }
    }
    return { source: 'log' as const, data: tail(this.logPath(attemptId), REPLAY_BYTES) };
  }

  onPaneExited(listener: (pane: string) => void) {
    this.paneExited.add(listener);
    this.ensureClient();
    return () => this.paneExited.delete(listener);
  }

  subscribe(pane: string, listener: TerminalListener) {
    const set = this.listeners.get(pane) ?? new Set(); set.add(listener); this.listeners.set(pane, set);
    this.ensureClient();
    return () => { set.delete(listener); if (!set.size) this.listeners.delete(pane); };
  }

  close() {
    this.closed = true; clearTimeout(this.retry);
    if (this.client) { try { this.client.stdin.write('detach-client\n'); } catch {} this.client.kill(); this.client = undefined; }
    this.listeners.clear();
    this.paneExited.clear();
  }
}

/** Follows a growing file (a running check's live output). */
export function followFile(path: string, listener: TerminalListener) {
  const initial = existsSync(path) ? tail(path, REPLAY_BYTES) : Buffer.alloc(0);
  let offset = existsSync(path) ? statSync(path).size : 0;
  let watcher: FSWatcher | undefined;
  const read = () => {
    if (!existsSync(path)) return;
    const size = statSync(path).size; if (size <= offset) return;
    const buffer = Buffer.alloc(size - offset); const fd = openSync(path, 'r');
    try { readSync(fd, buffer, 0, buffer.length, offset); } finally { closeSync(fd); }
    offset = size; listener(buffer);
  };
  const timer = setInterval(read, 500); timer.unref();
  try { watcher = watch(path, read); } catch { /* Created later; the interval picks it up. */ }
  return { initial, stop: () => { clearInterval(timer); watcher?.close(); } };
}

export const readRetained = (path: string) => existsSync(path) ? readFileSync(path) : Buffer.alloc(0);
