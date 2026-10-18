import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { existsSync } from 'node:fs';
import { z } from 'zod';
import { Conflict, NotFound, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import { ContainerCommand, type ContainerEngine } from './runtime/container-command';
import type { CheckOperation } from '../shared/evidence';

export class Checks {
  constructor(private store: Workbench, private work: RepositoryWork, private evidence: Evidence, private runtimeRoot: string, private engine: ContainerEngine) {}
  jobRoot(attemptId: string, toolCallId: string) { return join(this.runtimeRoot, attemptId, 'tools', createHash('sha256').update(toolCallId).digest('hex')); }
  async prepare(attemptId: string, toolCallId: string, input: unknown) {
    const { checkId } = z.object({ checkId: z.string() }).strict().parse(input); z.string().min(1).max(200).parse(toolCallId);
    const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
    // Rechecked after the snapshot: pause or cancel may be recorded while Git runs, and a prepared spec is launched at once.
    const authorize = () => {
      const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
      if (attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running'
        || !this.store.runtime(run.id).assignments.some(item => item.currentAttemptId === attemptId)) throw new Conflict('Only a current attempt may execute a check.');
    };
    authorize();
    if (this.work.unresolvedConflict(run.id)) throw new Conflict('Resolve the target conflict before running candidate checks.');
    const record = this.store.codingRecord(run.id); const candidate = record?.candidates.at(-1);
    const definition = record?.requiredChecks.find(item => item.id === checkId);
    if (!record || !candidate || !definition) throw new Conflict('Check or candidate is unavailable.');
    let operation = record.checkOperations?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (operation && operation.definition.id !== checkId) throw new Conflict('Check operation has different content.');
    if (!operation) {
      operation = { id: randomUUID(), attemptId, toolCallId, candidateId: candidate.id, definition, workspace: '', state: 'preparing' };
      this.store.updateCodingRecord(run.id, record => ({ ...record!, checkOperations: [...(record!.checkOperations ?? []), operation!] }));
    }
    if (operation.state === 'rejected') throw new Conflict(operation.reason ?? 'Check preparation was rejected.');
    if (operation.state === 'preparing') {
      try {
      // A new private snapshot preserves any incomplete copy after controller loss.
      const destination = join(this.jobRoot(attemptId, toolCallId), `source-${randomUUID()}`);
      const selected = record.candidates.find(item => item.id === operation!.candidateId)!;
      const workspace = await this.work.exportRevision(run.id, selected.revision, destination);
      const manifest = JSON.stringify(await this.work.listFiles(run.id));
      if (createHash('sha256').update(manifest).digest('hex') !== selected.revision.worktreeHash) throw new Conflict('Source manifest changed during check preparation.');
      authorize();
      operation = { ...operation, workspace, source: this.evidence.artifact(manifest), state: 'prepared' };
      const saved = operation;
      this.store.updateCodingRecord(run.id, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => item.id === saved.id ? saved : item) }));
      } catch (error) {
        if (error instanceof Conflict || error instanceof NotFound) {
          const id = operation.id;
          this.store.updateCodingRecord(run.id, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => item.id === id ? { ...item, state: 'rejected', reason: error.message } : item) }));
        }
        throw error;
      }
    }
    return { operationId: operation.id, candidateId: operation.candidateId, spec: { image: operation.definition.image, command: operation.definition.command, args: operation.definition.args, timeoutMs: operation.definition.timeoutMs, memoryMiB: operation.definition.memoryMiB, tmpExecutable: operation.definition.tmpExecutable, workspace: operation.workspace } };
  }
  async finish(attemptId: string, toolCallId: string) {
    const attempt = this.store.getAttempt(attemptId); const record = this.store.codingRecord(attempt.runId);
    const operation = record?.checkOperations?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (!record || !operation || ['preparing', 'rejected'].includes(operation.state)) throw new NotFound('Prepared check execution is unavailable.');
    const job = new ContainerCommand(this.jobRoot(attemptId, toolCallId), this.engine);
    if (operation.state === 'recorded') {
      await job.remove();
      return record.checks.find(item => item.id === operation.evidenceId)!;
    }
    const actual = await job.receipt(); const expected = operation.definition;
    if (actual.spec.workspace !== operation.workspace || actual.spec.image !== expected.image || actual.spec.command !== expected.command || JSON.stringify(actual.spec.args) !== JSON.stringify(expected.args) || actual.spec.timeoutMs !== expected.timeoutMs
      || (actual.spec.memoryMiB ?? 1024) !== (expected.memoryMiB ?? 1024)
      || (actual.spec.tmpExecutable ?? false) !== (expected.tmpExecutable ?? false)) throw new Conflict('Command receipt differs from the prepared execution.');
    if (!operation.source) throw new Conflict('Check source manifest is unavailable.');
    let sourceIntact = false;
    try { sourceIntact = this.work.verifyExport(operation.workspace, JSON.parse(this.evidence.readArtifact(operation.source))); } catch {}
    const receipt = this.evidence.recordCheck(attempt.runId, { candidateId: operation.candidateId, attemptId, checkId: expected.id, definition: expected,
      environment: { image: actual.spec.image, network: 'none', platform: actual.platform, memoryMiB: actual.spec.memoryMiB ?? 1024, tmpExecutable: actual.spec.tmpExecutable ?? false }, started: actual.started, completed: actual.completed, exitCode: actual.exitCode,
      timedOut: actual.timedOut, source: operation.source, sourceIntact, output: this.evidence.artifact(actual.output) }, operation.id);
    this.store.updateCodingRecord(attempt.runId, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => item.id === operation.id ? { ...item, state: 'recorded', evidenceId: receipt.id } : item) }));
    await job.remove();
    return receipt;
  }
  async reconcileStopped(runId: string) {
    for (const operation of this.store.codingRecord(runId)?.checkOperations ?? []) {
      if (operation.state === 'recorded') { await new ContainerCommand(this.jobRoot(operation.attemptId, operation.toolCallId), this.engine).remove(); continue; }
      if (operation.state === 'rejected' || this.store.getAttempt(operation.attemptId).ownership !== 'stopped') continue;
      if (operation.state === 'prepared' && existsSync(join(this.jobRoot(operation.attemptId, operation.toolCallId), 'container.json'))) {
        // Read the actual stopped command receipt. Unknown outcomes block continuation.
        await this.finish(operation.attemptId, operation.toolCallId);
      } else {
        this.store.updateCodingRecord(runId, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => item.id === operation.id ? { ...item, state: 'rejected', reason: 'Interrupted before command execution; no check evidence was produced.' } : item) }));
      }
    }
  }
  async execute(attemptId: string, toolCallId: string, input: unknown) {
    const prepared = await this.prepare(attemptId, toolCallId, input);
    const job = new ContainerCommand(this.jobRoot(attemptId, toolCallId), this.engine);
    if (!existsSync(join(job.root, 'container.json'))) await job.start(prepared.spec);
    await job.wait(); return this.finish(attemptId, toolCallId);
  }
}
