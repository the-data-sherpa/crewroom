import { resolve } from 'node:path';
import { readFileSync } from 'node:fs';
import { startServer } from './app';
import { loadJiraConnection } from './jira-connection';
import { loadPublicationConnections } from './publication-connection';

const port = Number(process.env.CREWROOM_PORT ?? 4317);
if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('CREWROOM_PORT must be a valid local port.');
const dataDir = resolve(process.env.CREWROOM_DATA_DIR ?? '.crewroom');
const server = await startServer({ port, dataDir, jira: loadJiraConnection(process.env.CREWROOM_JIRA_CONFIG ?? resolve(dataDir, 'jira.json')),
  publication: loadPublicationConnections(process.env.CREWROOM_PUBLICATION_CONFIG ?? resolve(dataDir, 'bitbucket.json')),
  production: process.argv.includes('--production'), rolePath: process.env.CREWROOM_ROLE_PATH,
  runtimeConfig: process.env.CREWROOM_RUNTIME_CONFIG ? JSON.parse(readFileSync(process.env.CREWROOM_RUNTIME_CONFIG, 'utf8')) : undefined });
console.log(`Crewroom listening on ${server.origin}`);
let closing = false;
for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, async () => {
  if (closing) return;
  closing = true;
  await server.close();
  process.exit(0);
});
