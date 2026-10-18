/** Test-only transport to the Rust process boundary. No workflow/provider effects. */
import { spawn, execFile, execFileSync, type ChildProcess, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import type { CheckedProcess, ControllerAuthority } from '../../server/runtime/processes';
export type { CheckedProcess, ControllerAuthority };
export const helper = join(resolve(process.env.CREWROOM_NATIVE_BINARY_DIRECTORY??'target/debug'),'crewroom-process');
type Options = { cwd: string; env: NodeJS.ProcessEnv };
const sync = (operation: string, input: unknown, extra: string[] = []) => JSON.parse(execFileSync(helper, [operation, ...extra], { input: JSON.stringify(input), encoding: 'utf8', timeout: 8000, stdio: ['pipe','pipe','pipe'] }));
const asyncCall = (operation: string, input?: unknown, extra: string[] = []): Promise<any> => new Promise((resolve, reject) => {
  const child = execFile(helper, [operation, ...extra], { timeout: 20000, maxBuffer: 1024 * 1024 }, (error, stdout, stderr) => {
    if (error) reject(new Error(stderr || error.message)); else { try { resolve(JSON.parse(stdout)); } catch (error) { reject(error); } }
  });
  child.stdin!.on('error', reject);
  child.stdin!.end(input === undefined ? '' : JSON.stringify(input));
});
export async function acquireController(root: string): Promise<ControllerAuthority> {
  if (basename(root) !== 'controller') throw new Error('Native fixture expects the controller subdirectory.');
  const holder = spawn(helper, ['hold', dirname(root)], { stdio: 'pipe' });
  let active = true; let stderr = ''; holder.stderr.on('data', b => stderr += b);
  const lost = new Promise<void>(resolve => holder.once('exit', () => { active = false; resolve(); }));
  const identity = await new Promise<{ root: string; epoch: string }>((resolve, reject) => {
    let raw = ''; holder.stdout.on('data', b => { raw += b; if (raw.includes('\n')) { try { resolve(JSON.parse(raw)); } catch (e) { reject(e); } } });
    holder.once('error', reject); holder.once('close', () => reject(new Error(stderr || 'Controller lock failed.')));
  });
  return { ...identity, lost, assertCurrent() {
    if (!active || readFileSync(join(identity.root, 'epoch.json'), 'utf8') !== JSON.stringify({ epoch: identity.epoch })) throw new Error('Controller authority lost.');
  }, async close() { holder.stdin.end(); await lost; } };
}
const input = (root: string, token: string, command: string, args: string[], options: Options, authority: ControllerAuthority) => ({ root, token, command, args, ...options, epoch: authority.epoch, controllerRoot: authority.root });
export function describeCheckedOwned(root: string, token: string, command: string, args: string[], options: Options, authority: ControllerAuthority): CheckedProcess {
  authority.assertCurrent(); return sync('describe', input(root, token, command, args, options, authority));
}
export function prepareCheckedOwned(root: string, token: string, command: string, args: string[], options: Options, authority: ControllerAuthority): CheckedProcess {
  authority.assertCurrent(); return sync('prepare-checked', input(root, token, command, args, options, authority));
}
export function launchCheckedOwned(descriptor: CheckedProcess, authority: ControllerAuthority): ChildProcess {
  authority.assertCurrent();
  sync('validate-launch', { descriptor, epoch: authority.epoch, controllerRoot: authority.root });
  return spawn(helper, ['checked-exec', descriptor.root, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { cwd: descriptor.root, env: { LANG: 'C', PATH: process.env.PATH }, detached: true, stdio: ['ignore', 'ignore', 'pipe'] });
}
export function launchOwned(root: string, command: string, args: string[], options: Options): ChildProcessWithoutNullStreams {
  sync('prepare-owned', { command, args, ...options }, [root]);
  return spawn(helper, ['exec', root], { cwd: options.cwd, env: { LANG: 'C', PATH: process.env.PATH }, detached: true, stdio: 'pipe' });
}
export const inspectOwned = (root: string) => asyncCall('inspect', undefined, [root]);
export const stopOwned = (root: string) => asyncCall('stop', undefined, [root]);
export const inspectCheckedOwned = (descriptor: CheckedProcess) => asyncCall('inspect-checked', descriptor);
export const sealCheckedOwned = (descriptor: CheckedProcess) => asyncCall('seal', descriptor);
