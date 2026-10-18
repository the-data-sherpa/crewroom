import { execFileSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

/**
 * Puts a `git` shim first on PATH. After `arm()`, exactly one later invocation of the `command` subcommand
 * blocks until `release()`; every other command runs the real Git unchanged.
 */
export function gitLatch(root: string, command = 'merge-base', skipMatches = 0) {
  const directory = join(root, 'git-latch'); const bin = join(directory, 'bin'); mkdirSync(bin, { recursive: true });
  const real = execFileSync('/bin/sh', ['-c', 'command -v git'], { encoding: 'utf8' }).trim();
  const quote = (value: string) => `'${value.replaceAll("'", `'\\''`)}'`;
  writeFileSync(join(bin, 'git'), `#!/bin/sh
state=${quote(directory)}
for argument in "$@"; do
  if [ "$argument" = ${quote(command)} ] && [ -e "$state/armed" ]; then
    count=$(cat "$state/skip")
    if [ "$count" -gt 0 ]; then
      printf '%s\\n' "$((count - 1))" > "$state/skip.tmp"
      mv "$state/skip.tmp" "$state/skip"
      break
    fi
    if mv "$state/armed" "$state/claimed" 2>/dev/null; then
      printf '%s\n' "$$" > "$state/pid"
      : > "$state/blocked"
      while [ ! -e "$state/release" ]; do sleep 0.01; done
      if [ -e "$state/capture" ]; then
        ${quote(real)} "$@" > "$state/git.stdout" 2> "$state/git.stderr"
      else
        ${quote(real)} "$@" 2> "$state/git.stderr"
      fi
      result=$?
      printf '%s\n' "$result" > "$state/git.exit"
      : > "$state/done"
      exit "$result"
    fi
  fi
done
exec ${quote(real)} "$@"
`);
  chmodSync(join(bin, 'git'), 0o755);
  const previous = process.env.PATH; process.env.PATH = `${bin}:${previous}`;
  return {
    arm() {
      for (const name of ['claimed', 'blocked', 'release', 'skip', 'skip.tmp', 'pid', 'done', 'capture', 'git.stdout', 'git.stderr', 'git.exit']) rmSync(join(directory, name), { force: true });
      writeFileSync(join(directory, 'skip'), String(skipMatches));
      writeFileSync(join(directory, 'armed'), '');
    },
    async blocked() {
      const deadline = Date.now() + 10000;
      while (!existsSync(join(directory, 'blocked'))) {
        if (Date.now() > deadline) throw new Error('The merge-base probe never started.');
        await delay(5);
      }
    },
    pid() { return Number(readFileSync(join(directory, 'pid'), 'utf8').trim()); },
    async finished() {
      const deadline = Date.now() + 10000;
      while (!existsSync(join(directory, 'done'))) {
        if (Date.now() > deadline) throw new Error('The released Git helper did not finish.');
        await delay(5);
      }
      const pid = Number(readFileSync(join(directory, 'pid'), 'utf8').trim());
      for (;;) {
        try { process.kill(pid, 0); }
        catch (error) {
          if (error && typeof error === 'object' && 'code' in error && error.code === 'ESRCH') return;
          throw error;
        }
        if (Date.now() > deadline) throw new Error('The released Git helper still has a process.');
        await delay(5);
      }
    },
    release() { writeFileSync(join(directory, 'release'), ''); },
    restore() { process.env.PATH = previous; },
  };
}
