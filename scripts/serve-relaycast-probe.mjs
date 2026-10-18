import { createEngine } from '@relaycast/engine';
import { createNodeRuntime } from '@relaycast/engine/adapters/node';
import { serve } from '@hono/node-server';
import path from 'node:path';

const port = Number(process.env.CREWROOM_RELAY_PORT || 18787);
const base = process.env.CREWROOM_RELAY_DATA || path.resolve('data');
const runtime = createNodeRuntime({
  dbPath: path.join(base, 'relaycast.db'),
  baseUrl: `http://127.0.0.1:${port}`,
  fileDir: path.join(base, 'files'),
  config: { environment: 'crewroom-experiment' },
});
const engine = createEngine(runtime.deps);
const server = serve({ fetch: engine.fetch, hostname: '127.0.0.1', port }, () => {
  console.log(JSON.stringify({ event: 'listening', host: '127.0.0.1', port, pid: process.pid }));
});
let stopping = false;
function stop() {
  if (stopping) return;
  stopping = true;
  server.close(() => { runtime.close(); process.exit(0); });
}
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
