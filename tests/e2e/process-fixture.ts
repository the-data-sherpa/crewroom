import { spawn } from 'node:child_process';
import { once } from 'node:events';

export async function launch(dataDir: string) {
  const child = spawn(process.execPath, ['--import', 'tsx', 'server/index.ts', '--production'], {
    env: { ...process.env, CREWROOM_PORT: '0', CREWROOM_DATA_DIR: dataDir, CREWROOM_ROLE_PATH: `${dataDir}/roles.json` },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  const origin = await new Promise<string>((resolve, reject) => {
    const deadline = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Server start timeout: ${output}`)); }, 15000);
    child.once('error', error => { clearTimeout(deadline); reject(error); });
    child.once('exit', () => { clearTimeout(deadline); reject(new Error(`Server exited: ${output}`)); });
    child.stderr.on('data', chunk => { output += chunk; });
    child.stdout.on('data', chunk => {
      output += chunk;
      const match = output.match(/Crewroom listening on (http:\/\/127\.0\.0\.1:\d+)/);
      if (match) { clearTimeout(deadline); resolve(match[1]); }
    });
  });
  return { origin, async close() {
    if (child.exitCode !== null || child.signalCode !== null) return;
    const exited = once(child, 'exit');
    child.kill('SIGTERM');
    const deadline = setTimeout(() => child.kill('SIGKILL'), 5000);
    try { await exited; } finally { clearTimeout(deadline); }
  } };
}
