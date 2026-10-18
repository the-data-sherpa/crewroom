import { writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import assert from 'node:assert/strict';

/** Real backend process for crash-boundary tests; all requests still use its HTTP API. */
export async function backendFixture(config: any, options: { existingRuntime?: boolean } = {}) {
  const runtimePath = join(config.root, 'runtime-config.json');
  if (!options.existingRuntime) writeFileSync(runtimePath, JSON.stringify(config.runtime), { mode: 0o600 });
  let child: ChildProcess;
  const launch = async () => {
    child = spawn(process.execPath, ['--import', 'tsx', resolve('server/index.ts'), '--production'], { stdio: 'pipe', env: { ...process.env,
      CREWROOM_DATA_DIR: config.root, CREWROOM_ROLE_PATH: config.rolePath, CREWROOM_PORT: String(config.serverPort), CREWROOM_RUNTIME_CONFIG: runtimePath } });
    let log = ''; child.stdout!.on('data', chunk => log += chunk); child.stderr!.on('data', chunk => log += chunk);
    const deadline = Date.now() + 30000;
    while (!log.includes('Crewroom listening') && child.exitCode === null && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 30));
    assert.ok(log.includes('Crewroom listening'), log);
  };
  await launch();
  return { origin: `http://127.0.0.1:${config.serverPort}`,
    get pid() { return child.pid; },
    freeze() { child.kill('SIGSTOP'); },
    unfreeze() { child.kill('SIGCONT'); },
    async restart() { const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGKILL'); await exited; await launch(); },
    async close() { if (child.exitCode === null && child.signalCode === null) { child.kill('SIGCONT'); const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM'); await exited; } },
  };
}
