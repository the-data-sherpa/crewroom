import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

process.env.CREWROOM_E2E_DATA ??= mkdtempSync(join(tmpdir(), 'crewroom-browser-'));
export default defineConfig({
  testDir: './tests/e2e', workers: 1, fullyParallel: false,
  globalTeardown: './tests/e2e/cleanup.ts',
  use: { baseURL: 'http://127.0.0.1:4331', trace: 'retain-on-failure' },
  webServer: { command: 'npx tsx server/index.ts --production', url: 'http://127.0.0.1:4331/api/workspace',
    reuseExistingServer: false, gracefulShutdown: { signal: 'SIGTERM', timeout: 5000 },
    env: { CREWROOM_PORT: '4331', CREWROOM_DATA_DIR: process.env.CREWROOM_E2E_DATA,
      CREWROOM_ROLE_PATH: join(process.env.CREWROOM_E2E_DATA, 'roles.json') } },
});
