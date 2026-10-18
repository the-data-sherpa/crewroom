/** Production-daemon preparation controls. No model inference or actor input.
 * A proxy holds one genuine completed Docker log response before admission.
 * Opt in only with the isolated fixture slot available.
 */
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import { NativeDaemon } from './daemon-client';
import { comparisonSettled } from './for14-stop-state';
import { until, workflowFixture } from './workflow-support';

const imageReceipt = process.env.CREWROOM_FOR14_CHECK_IMAGE_RECEIPT ?? resolve('.crewroom/test-results/for14/check-image-v1-final/image-receipt.json');
const skip = process.env.CREWROOM_FOR14_PREPARATION_EXECUTE !== '1'
  ? 'Explicitly release the isolated fixture slot before executing this regression.'
  : !existsSync(imageReceipt) ? 'The protected check image receipt is required.' : false;

for (const action of ['cancel', 'pause'] as const) {
  test(`FOR-14 ${action} during preparation settles without admitting an actor`, { timeout: 120000, skip }, async t => {
    let releaseLog = () => {};
    const gate = new Promise<void>(resolve => { releaseLog = resolve; });
    let heldUrl: string | undefined;
    let intercepted = false;
    let engineSocket = '';
    let proxy: ReturnType<typeof createServer> | undefined;
    t.after(() => releaseLog());
    const parent = process.env.CREWROOM_FOR14_PREPARATION_ROOT;
    if (parent) mkdirSync(parent, { recursive: true, mode: 0o700 });
    const f = await workflowFixture(t, 'coding', 0, false, {
      service: true, ...(parent ? { parent } : {}), cleanup: () => {},
      configure: async ({ root, config }) => {
        const executable = join(root, 'scripted-omp-identity'); copyFileSync('/usr/bin/false', executable);
        config.runtime.ompBinary = executable; config.runtime.evaluationMode = 'scripted';
        config.runtime.evaluationImage = JSON.parse(readFileSync(imageReceipt, 'utf8')).image;
        engineSocket = config.runtime.commandEngine.socket;
        proxy = createServer((req, res) => {
          const hold = !intercepted && !!req.url?.includes('/logs?');
          if (hold) intercepted = true;
          const upstream = request({ socketPath: engineSocket, path: req.url, method: req.method, headers: req.headers }, response => {
            if (hold) {
              const chunks: Buffer[] = []; response.on('data', chunk => chunks.push(Buffer.from(chunk)));
              response.on('end', () => {
                heldUrl = req.url;
                void gate.then(() => {
                  if (!res.destroyed) { res.writeHead(response.statusCode!, response.headers); res.end(Buffer.concat(chunks)); }
                });
              });
            } else { res.writeHead(response.statusCode!, response.headers); response.pipe(res); }
          });
          upstream.on('error', () => res.destroy()); req.pipe(upstream); res.on('close', () => upstream.destroy());
        });
        const socket = join(root, 'engine.sock'); proxy.listen(socket); await once(proxy, 'listening');
        config.runtime.commandEngine.socket = socket;
      },
    });
    t.after(() => { proxy?.closeAllConnections(); proxy?.close(); });
    assert.ok(f.c instanceof NativeDaemon); const c = f.c;
    let comparisonId: string | undefined;
    let runId: string | undefined;
    const receipt: any = { mode: 'scripted', inference: 'none', action, passed: false, phase: 'preflight' };
    const save = () => writeFileSync(join(f.root, 'for14-preparation-receipt.json'), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
    const get = () => c.request(`/api/comparisons/${comparisonId}`);
    const control = (action: string) => c.request(`/api/comparisons/${comparisonId}/control`, { requestId: randomUUID(), action });
    const runtime = () => c.request(`/api/runs/${runId}/runtime`);
    try {
      const current = (await c.request('/api/workspace')).roles.find((role: any) => role.id === 'implementer');
      const draft = { ...current, revision: current.revision ?? 1, instructions: current.instructions + ' Preserve incomplete verification honestly.' };
      const preview = await c.request('/api/comparisons/preflight', { kind: 'role', targetId: 'implementer', baseRevision: draft.revision, draft });
      assert.equal(preview.canStart, true, JSON.stringify(preview)); assert.equal(preview.manifest.mode, 'scripted');
      const comparison = await c.request('/api/comparisons', { requestId: randomUUID(), manifest: preview.manifest });
      comparisonId = comparison.id;
      receipt.comparisonId = comparisonId; receipt.manifestHash = comparison.manifestHash;
      await control('start');
      const before = await until(async () => { const value = await get(); return heldUrl && value.cells[0].runId ? value : false; }, 30000);
      runId = before.cells[0].runId;
      const intentPath = join(f.runtime, runId!, 'tools', 'image-proof', 'container.json');
      const intent = JSON.parse(readFileSync(intentPath, 'utf8'));
      assert.equal(intent.startConfirmed, true); assert.equal(intent.completed.exitCode, 0);
      assert.equal(heldUrl, `/containers/${intent.id}/logs?stdout=true&stderr=true`);
      const initialRuntime = await runtime(); assert.equal(initialRuntime.attempts.length, 0); assert.equal(initialRuntime.assignments.length, 0);
      const controlled = await control(action), queue = (await c.request('/api/workspace')).queue;
      receipt.phase = 'preparation-held'; receipt.runId = runId;
      receipt.barrier = { kind: 'actual-completed-image-proof-log-response', intentPath, containerId: intent.id, heldUrl };
      receipt.before = before; receipt.controlled = controlled; receipt.controlledQueue = queue; receipt.initialRuntime = initialRuntime; save();
      assert.equal(controlled.desiredState, action === 'cancel' ? 'cancelled' : 'paused');
      assert.equal(controlled.state, action === 'cancel' ? 'cancelling' : 'paused');
      assert.equal(controlled.cells[0].result, undefined); assert.equal(queue.slot.runId, runId);
      assert.equal(comparisonSettled(controlled, queue), false);
      releaseLog();
      if (action === 'pause') {
        await until(async () => { const value = await runtime(); return value.run.status === 'paused' && !value.busy ? value : false; }, 30000);
        const paused = await get(); receipt.paused = paused; save();
        assert.equal(paused.state, 'paused'); assert.equal(paused.cells[0].result, undefined);
        assert.equal(paused.cells[0].attempts.length, 0); assert.equal(paused.activeMs, 0);
        assert.equal((await c.request('/api/workspace')).queue.slot.runId, runId);
        await control('cancel');
      }
      const settled = await until(async () => {
        const value = await get(), workspace = await c.request('/api/workspace');
        return comparisonSettled(value, workspace.queue) ? { comparison: value, queue: workspace.queue } : false;
      }, 30000);
      receipt.phase = 'settled-before-close'; receipt.settled = settled; save();
      const cell = settled.comparison.cells[0];
      assert.equal(settled.comparison.state, 'cancelled'); assert.equal(cell.result.state, 'cancelled'); assert.equal(cell.result.observed, false);
      assert.equal(cell.attempts.length, 0); assert.equal(settled.comparison.activeMs, 0);
      assert.ok(settled.comparison.cells.slice(1).every((next: any) => !next.runId));
      assert.equal((await runtime()).assignments.length, 0);
      assert.equal(readdirSync(f.runtime).filter(name => existsSync(join(f.runtime, name, 'host-intent.json'))).length, 0);
      await c.close(); assert.equal(c.proc.exitCode, 0); assert.equal(c.proc.signalCode, null);
      const db = new DatabaseSync(f.database, { readOnly: true });
      try {
        receipt.afterClose = {
          slot: db.prepare('SELECT run_id FROM coding_slot WHERE id=1').get(),
          run: JSON.parse(String(db.prepare('SELECT data FROM runs WHERE id=?').get(runId!)!.data)),
          cell: JSON.parse(String(db.prepare('SELECT data FROM comparison_cells WHERE id=?').get(cell.id)!.data)),
          agentEventCount: db.prepare('SELECT COUNT(*) AS count FROM agent_events e JOIN attempts a ON a.id=e.attempt_id JOIN assignments s ON s.id=a.assignment_id WHERE s.run_id=?').get(runId!),
        };
      } finally { db.close(); }
      assert.equal(receipt.afterClose.slot.run_id, null); assert.equal(receipt.afterClose.run.status, 'cancelled');
      assert.equal(receipt.afterClose.cell.resultHash, cell.resultHash); assert.equal(receipt.afterClose.agentEventCount.count, 0);
      receipt.containers = [];
      const commands = join(f.runtime, runId!, 'tools');
      for (const name of readdirSync(commands)) {
        const path = join(commands, name, 'container.json');
        if (!existsSync(path)) continue;
        const stoppedIntent = JSON.parse(readFileSync(path, 'utf8'));
        assert.equal(stoppedIntent.removed, true, `Unremoved owned preparation command: ${name}`);
        const status = await new Promise<number>((resolve, reject) => {
          const req = request({ socketPath: engineSocket, path: `/containers/${stoppedIntent.id}/json`, method: 'GET' }, res => { res.resume(); res.on('end', () => resolve(res.statusCode!)); });
          req.on('error', reject); req.setTimeout(10000, () => req.destroy(new Error('Bounded container verification timed out'))); req.end();
        });
        assert.equal(status, 404); receipt.containers.push({ path, intent: stoppedIntent, absentStatus: status });
      }
      assert.ok(receipt.containers.some((container: any) => container.intent.id === intent.id));
      receipt.phase = 'stopped'; receipt.passed = true;
    } catch (error) { receipt.error = String(error); throw error; }
    finally {
      releaseLog();
      try {
        if (comparisonId && c.proc.exitCode === null && c.proc.signalCode === null) {
          const value = await get();
          if (!['completed', 'incomplete', 'cancelled'].includes(value.state) && value.desiredState !== 'cancelled') await control('cancel');
          await until(async () => {
            const value = await get(), workspace = await c.request('/api/workspace');
            return comparisonSettled(value, workspace.queue) ? value : false;
          }, 30000);
        }
      } catch (error) { receipt.cleanupError = String(error); receipt.passed = false; }
      await c.close(); receipt.daemonExitCode = c.proc.exitCode; receipt.daemonSignal = c.proc.signalCode; save();
    }
  });
}
