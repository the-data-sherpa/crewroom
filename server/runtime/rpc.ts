import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { createInterface } from 'node:readline';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { launchOwned, stopOwned } from './processes';

export class OmpRpc {
  readonly child: ChildProcessWithoutNullStreams;
  readonly ownershipRoot: string;
  private pending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  constructor(command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv }, private event: (frame: any) => void) {
    this.ownershipRoot = join(options.cwd, 'omp-process');
    this.child = launchOwned(this.ownershipRoot, command, args, options);
    this.child.stdin.on('error', error => this.fail(error));
    this.child.stderr.on('data', () => { /* Private runner stderr is not browser activity. */ });
    createInterface({ input: this.child.stdout }).on('line', line => {
      let frame; try { frame = JSON.parse(line); } catch { return; }
      if (frame.type === 'response' && this.pending.has(frame.id)) {
        const request = this.pending.get(frame.id)!; this.pending.delete(frame.id); clearTimeout(request.timer);
        if (frame.success) request.resolve(frame.data); else request.reject(new Error(`OMP rejected ${frame.command}.`));
      } else this.event(frame);
    });
    this.child.once('error', error => this.fail(error));
    this.child.once('exit', (code, signal) => {
      this.fail(new Error('OMP process exited.'));
      this.event({ type: 'process_exit', code, signal });
    });
  }
  private fail(error: Error) {
    for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); }
    this.pending.clear();
  }
  send(frame: unknown) { if (!this.child.stdin.destroyed && !this.child.stdin.writableEnded) this.child.stdin.write(JSON.stringify(frame) + '\n'); }
  command(type: string, args: Record<string, unknown> = {}, id: string = randomUUID()): Promise<any> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`OMP ${type} timed out.`)); }, 20000);
      this.pending.set(id, { resolve, reject, timer });
      this.send({ ...args, type, id });
    });
  }
  async close() {
    if (this.child.exitCode === null && this.child.signalCode === null) {
      await Promise.race([this.command('abort').catch(() => {}), new Promise(resolve => setTimeout(resolve, 500))]);
      this.child.stdin.end();
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    const ownership = await stopOwned(this.ownershipRoot);
    if (ownership.state !== 'stopped') throw new Error(ownership.reason ?? 'OMP ownership remains uncertain.');
  }
}
