import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative, resolve } from 'node:path';
import { test } from 'node:test';
import { createServer } from 'vite';

// FOR-59: the dev-mode UI middleware that browser fixtures start must not watch disposable worktrees or build output.
test('FOR-59: the UI watcher skips nested worktrees and build output but still watches the UI source', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-watch-'));
  const unwatched = ['.claude/worktrees/first/src', '.claude/worktrees/second/src', '.crewroom/mh2-omp-fixture', 'target/debug', 'dist/assets', 'crates/core/src'];
  for (const directory of ['src', ...unwatched]) {
    mkdirSync(join(root, directory), { recursive: true });
    writeFileSync(join(root, directory, 'module.ts'), 'export {};\n');
  }
  writeFileSync(join(root, 'index.html'), '<!doctype html><script type="module" src="/src/module.ts"></script>\n');
  const server = await createServer({ configFile: resolve('vite.config.ts'), root, logLevel: 'silent',
    server: { middlewareMode: true, hmr: false, ws: false } });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  await new Promise<void>(done => { server.watcher.once('ready', done); setTimeout(done, 3000); });
  const watched = Object.keys(server.watcher.getWatched()).map(path => relative(root, path));
  assert.ok(watched.includes('src'), `UI source must stay watched: ${watched.join(', ')}`);
  for (const directory of unwatched) {
    const top = directory.split('/')[0];
    assert.ok(!watched.some(path => path === directory || path.startsWith(`${directory}/`) || (path.startsWith(`${top}/`))),
      `${directory} must not be watched: ${watched.join(', ')}`);
  }
});
