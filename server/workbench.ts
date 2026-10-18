import { DatabaseSync } from 'node:sqlite';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { z } from 'zod';
import { continuationRequest } from './continuation-request';
import { descriptionText, jiraImportSchema, jiraIssueSchema } from './jira';
import type { PublicationRecord, FeedbackObservation } from '../shared/publication';
import type { DeliveryNotice } from '../shared/delivery';
import type { BitbucketCommentRead } from './bitbucket-client';
import type { JiraSource, JiraSyncState, JiraUpdate } from '../shared/jira';
import type { CodingQueue, LaunchSpec, Message, Note, Repository, RequirementRevision, Role, Run, Task } from '../shared/types';
import type { AssignmentModel } from './runtime/model-policy';
import { insertionRequest, type InsertionRequest } from '../shared/insertion';
import type { Assignment, Attempt, PeerMessage, ProposedSpecialist, TeamProposal } from './runtime/types';
import { snapshotBriefs, type AssignmentBrief } from './runtime/brief';
import { automaticCycleLimit, cyclesExhausted, pendingMessageLimit } from './runtime/types';
import { roleSchema } from './roles';
import type { CodingRecord, ImplementationClaim } from '../shared/evidence';
import type { LearningProposal } from '../shared/learning';
import type { RefinementProposal } from '../shared/refinement';
import type { ResearchArtifact } from '../shared/research';
import type { RepositoryWorkRecord, RepositoryCheckpoint, TargetRefresh, TargetRefreshWorkspace, TargetConflictContinuation, TargetConflictContinuationResult } from '../shared/repository';
import { requirementsHash } from '../shared/requirements';
import { compactObservation } from './runtime/observation';
import { repositorySchema, repositoryUpdateSchema, type ValidatedRepository } from './repository-configuration';
import { transitionRun, LifecycleConflict } from './run-lifecycle';
import { dependencyDelivery, dependencyHold, executionConflict, type ExecutingRun } from './execution-policy';
import { parseLaunch } from './run-launch';
import { steeringClosedReason, steeringTransitions, steeringUndeliveredReason } from '../shared/steering';
import { hostedCiPending } from './active-time';


export class Conflict extends Error {}
export class FeedbackResumeConflict extends Conflict {
  constructor(message: string, readonly terminal: boolean) { super(message); }
}
export class NotFound extends Error {}
export class RepositoryConfigurationChanged extends Conflict {}
interface UnpromotedConflictProof {
  originalHead: string; originalBase: string; originalPath: string; originalBranch: string; originalTargetRef: string; targetRef: string;
  originalGitFile: string; workspaceGitFile: string; workspacePath: string; workspaceBranch: string; toHead: string; worktreeHash: string;
  requirementsHash: string; instructionsHash: string; expectedTarget: string;
}

const refinementSchema = z.object({
  revision: z.number().int().positive(), sourceRevision: z.number().int().positive(), brief: z.string().max(40000),
  criteria: z.array(z.string().trim().min(1).max(2000)).max(100),
  decisionNote: z.string().trim().min(1).max(4000).optional(),
});
const proposalSchema = z.object({ brief: z.string().trim().min(1).max(40000),
  criteria: z.array(z.string().trim().min(1).max(2000)).min(1).max(100),
  decisions: z.array(z.string().trim().min(1).max(4000)).max(100),
  unresolvedQuestions: z.array(z.string().trim().min(1).max(4000)).max(100) }).strict();

export class Workbench {
  private db: DatabaseSync;
  private closed = false;
  constructor(path: string, private clock: () => number = Date.now) {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    this.db = new DatabaseSync(path);
    chmodSync(path, 0o600);
    this.db.exec('PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;');
    const version = Number(this.db.prepare('PRAGMA user_version').get()!.user_version);
    if (version > 26) { this.close(); throw new Error('Workflow database is newer than this application.'); }
    if (version === 0) this.transaction(() => {
      this.db.exec(`CREATE TABLE tickets (
        id TEXT PRIMARY KEY, source_site TEXT, source_id TEXT,
        imported TEXT NOT NULL, refinement TEXT NOT NULL,
        UNIQUE(source_site, source_id)
      ); PRAGMA user_version=1;`);
    });
    if (version < 2) this.transaction(() => {
      this.db.exec(`CREATE TABLE repositories (id TEXT PRIMARY KEY, data TEXT NOT NULL);
        CREATE TABLE runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
        CREATE UNIQUE INDEX one_plan_per_ticket ON runs(task_id);
        PRAGMA user_version=2;`);
    });
    if (version < 3) this.transaction(() => {
      this.db.exec(`CREATE TABLE notes (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
        CREATE TABLE steering (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), request_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, request_id));
        PRAGMA user_version=3;`);
    });
    if (version < 4) this.transaction(() => {
      this.db.exec(`CREATE TABLE assignments (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), name TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, name));
        CREATE TABLE attempts (id TEXT PRIMARY KEY, assignment_id TEXT NOT NULL REFERENCES assignments(id), data TEXT NOT NULL);
        CREATE TABLE peer_messages (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
        PRAGMA user_version=4;`);
    });
    if (version < 5) this.transaction(() => {
      this.db.exec(`CREATE TABLE observer_cursors (assignment_id TEXT PRIMARY KEY REFERENCES assignments(id), cursor INTEGER NOT NULL);
        CREATE TABLE observations (assignment_id TEXT NOT NULL REFERENCES assignments(id), cursor INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(assignment_id,cursor));
        PRAGMA user_version=5;`);
    });
    if (version < 6) this.transaction(() => {
      this.db.exec(`CREATE TABLE agent_events (attempt_id TEXT NOT NULL REFERENCES attempts(id), sequence INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(attempt_id,sequence));
        PRAGMA user_version=6;`);
    });
    if (version < 7) this.transaction(() => {
      this.db.exec(`CREATE TABLE team_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id)); PRAGMA user_version=7;`);
    });
    if (version < 8) this.transaction(() => {
      this.db.exec('CREATE TABLE repository_work (run_id TEXT PRIMARY KEY REFERENCES runs(id), common_dir TEXT NOT NULL, branch TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(common_dir,branch)); PRAGMA user_version=8;');
    });
    if (version < 9) this.transaction(() => {
      this.db.exec('CREATE TABLE coding (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL); PRAGMA user_version=9;');
    });
    if (version < 10) this.transaction(() => { this.db.exec('CREATE TABLE jira_sync (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL); PRAGMA user_version=10;'); });
    if (version < 11) this.transaction(() => { this.db.exec('CREATE TABLE jira_updates (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), event_key TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id,event_key)); PRAGMA user_version=11;'); });
    if (version < 12) this.transaction(() => { this.db.exec('CREATE TABLE publications (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL); PRAGMA user_version=12;'); });
    if (version < 13) this.transaction(() => { this.db.exec('CREATE TABLE delivery_notices (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), data TEXT NOT NULL); PRAGMA user_version=13;'); });
    if (version < 14) this.transaction(() => {
      const update = this.db.prepare('UPDATE agent_events SET data=? WHERE attempt_id=? AND sequence=?');
      for (const row of this.db.prepare("SELECT attempt_id, sequence, data FROM agent_events WHERE json_extract(data,'$.type')='message_update'").iterate()) {
        update.run(JSON.stringify(compactObservation(JSON.parse(String(row.data)))), row.attempt_id, row.sequence);
      }
      this.db.exec("CREATE INDEX IF NOT EXISTS agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type')); PRAGMA user_version=14;");
    });
    if (version < 15) this.transaction(() => {
      // Generated columns cannot drift from the retained JSON records. Each step is idempotent so a
      // partially migrated or rewound database (user_version lowered by hand) migrates cleanly.
      const columns = (table: string) => new Set(this.db.prepare(`PRAGMA table_xinfo(${table})`).all().map(row => String(row.name)));
      const add = (table: string, column: string, path: string) => {
        if (!columns(table).has(column)) this.db.exec(`ALTER TABLE ${table} ADD COLUMN ${column} TEXT GENERATED ALWAYS AS (json_extract(data,'$.${path}')) VIRTUAL`);
      };
      add('runs', 'status', 'status'); add('runs', 'desired_state', 'desiredState');
      add('attempts', 'state', 'state'); add('attempts', 'ownership', 'ownership');
      this.db.exec(`CREATE INDEX IF NOT EXISTS runs_lifecycle ON runs(status, desired_state);
        CREATE INDEX IF NOT EXISTS attempts_lifecycle ON attempts(state, ownership);`);
      if (!columns('agent_events').has('cursor')) this.db.exec(`CREATE TABLE agent_events_v15 (cursor INTEGER PRIMARY KEY AUTOINCREMENT, attempt_id TEXT NOT NULL REFERENCES attempts(id), sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,sequence));
        INSERT INTO agent_events_v15(cursor,attempt_id,sequence,data) SELECT rowid,attempt_id,sequence,data FROM agent_events ORDER BY rowid;
        DROP TABLE agent_events;
        ALTER TABLE agent_events_v15 RENAME TO agent_events;`);
      this.db.exec("CREATE INDEX IF NOT EXISTS agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type')); PRAGMA user_version=15;");
      // Old histories are compacted only when an exact, durable final receipt proves completion.
      for (const row of this.db.prepare("SELECT attempt_id, sequence, data FROM agent_events WHERE json_extract(data,'$.type')='message_end' ORDER BY cursor").all()) {
        this.compactCompletedMessage(String(row.attempt_id), Number(row.sequence), JSON.parse(String(row.data)));
      }
    });
    if (version < 16) this.transaction(() => {
      this.db.exec(`CREATE TABLE IF NOT EXISTS learning_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id));
        PRAGMA user_version=16;`);
    });
    if (version < 17) this.transaction(() => {
      this.db.exec(`CREATE TABLE IF NOT EXISTS requirement_revisions (
        task_id TEXT NOT NULL REFERENCES tickets(id), revision INTEGER NOT NULL, data TEXT NOT NULL,
        PRIMARY KEY(task_id, revision)
      ); PRAGMA user_version=17;`);
      const insert = this.db.prepare('INSERT OR IGNORE INTO requirement_revisions VALUES(?,?,?)');
      for (const row of this.db.prepare('SELECT id, imported, refinement FROM tickets').all()) {
        const source = JSON.parse(String(row.imported)); const refinement = JSON.parse(String(row.refinement));
        const run = this.db.prepare('SELECT data FROM runs WHERE task_id=? ORDER BY rowid DESC LIMIT 1').get(row.id);
        // v16 did not retain the accepted source revision. For unresolved refreshes the admitted
        // snapshot is the last durable source identity available; otherwise the current source is accepted.
        const acceptedSourceRevision = refinement.acceptedSourceRevision ?? (!refinement.needsReacceptance ? source.sourceRevision
          : (run ? (JSON.parse(String(run.data)) as Run).taskSnapshot.sourceRevision : undefined) ?? Math.max(1, source.sourceRevision - 1));
        const record: RequirementRevision = { taskId: String(row.id), revision: refinement.revision,
          brief: refinement.brief, criteria: refinement.criteria, sourceRevision: acceptedSourceRevision,
          origin: 'operator', createdAt: source.updatedAt };
        insert.run(record.taskId, record.revision, JSON.stringify(record));
      }
    });
    if (version < 18) this.transaction(() => {
      this.db.exec(`DROP INDEX IF EXISTS one_plan_per_ticket;
        CREATE UNIQUE INDEX IF NOT EXISTS one_open_run_per_ticket ON runs(task_id) WHERE status NOT IN ('cancelled','closed');
        CREATE TABLE IF NOT EXISTS refinement_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id),
          attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL,
          data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
        PRAGMA user_version=18;`);
    });
    if (version < 19) this.transaction(() => {
      this.db.exec(`DROP INDEX IF EXISTS one_open_run_per_ticket;
        CREATE UNIQUE INDEX IF NOT EXISTS one_open_run_per_ticket ON runs(task_id) WHERE status NOT IN ('cancelled','closed','completed');
        CREATE TABLE IF NOT EXISTS research_artifacts (
          id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id),
          tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id)
        ); PRAGMA user_version=19;`);
    });
    if (version < 20) this.transaction(() => {
      const migrated = !!this.db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='queue_state'").get();
      this.db.exec(`CREATE TABLE IF NOT EXISTS queue_state (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO queue_state VALUES(1,0);
        CREATE TABLE IF NOT EXISTS queue_requests (request_id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);`);
      if (!migrated) {
        const rows = this.db.prepare('SELECT data FROM runs ORDER BY rowid').all();
        const queued = rows.map((row, rowid) => ({ run: JSON.parse(String(row.data)) as Run, rowid }))
          .filter(({ run }) => (run.purpose ?? 'coding') === 'coding' && run.status === 'awaiting_runner'
            && !this.db.prepare('SELECT 1 FROM assignments WHERE run_id=? LIMIT 1').get(run.id))
          .sort((a, b) => a.run.createdAt.localeCompare(b.run.createdAt) || a.rowid - b.rowid);
        const positions = new Map(queued.map(({ run }, index) => [run.id, index]));
        for (const row of rows) {
          const run = JSON.parse(String(row.data)) as Run;
          run.position = positions.get(run.id);
          this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(run), run.id);
        }
      }
      this.db.exec('PRAGMA user_version=20;');
    });
    if (version < 21) this.transaction(() => {
      this.db.exec(`CREATE TABLE IF NOT EXISTS coding_slot (id INTEGER PRIMARY KEY CHECK(id=1), run_id TEXT REFERENCES runs(id));
        INSERT OR IGNORE INTO coding_slot(id,run_id) VALUES(1,NULL);`);
      const candidates = this.db.prepare(`SELECT r.data FROM runs r WHERE COALESCE(json_extract(r.data,'$.purpose'),'coding')='coding'
        AND r.status NOT IN ('cancelled','closed','completed') AND json_extract(r.data,'$.position') IS NULL AND EXISTS
        (SELECT 1 FROM assignments a WHERE a.run_id=r.id UNION SELECT 1 FROM repository_work w WHERE w.run_id=r.id) ORDER BY r.rowid`).all()
        .map(row => JSON.parse(String(row.data)) as Run).filter(run => !this.publication(run.id)?.merged);
      const claimed = candidates.filter(run => this.db.prepare(`SELECT 1 FROM attempts a JOIN assignments s ON s.id=a.assignment_id
        WHERE s.run_id=? AND a.state<>'admitted' AND a.ownership IS NOT 'stopped' LIMIT 1`).get(run.id));
      const holder = candidates.length === 1 ? candidates[0] : claimed[0];
      for (const run of candidates) {
        if (run.id === holder?.id) {
          run.schedule ??= { id: randomUUID(), at: run.createdAt };
          run.launchCompletedAt ??= run.createdAt;
          this.db.prepare('UPDATE coding_slot SET run_id=? WHERE id=1').run(run.id);
        } else {
          run.slotRelease ??= { reason: 'migration', at: new Date(this.clock()).toISOString() };
          if (claimed.some(item => item.id === run.id)) {
            run.status = 'attention_required'; run.desiredState = 'paused';
            run.attentionReason = 'Another retained coding run owns the slot. Reconcile this run’s process ownership before resuming.';
          } else if (run.status === 'running' || run.status === 'awaiting_runner') {
            run.status = 'paused'; run.desiredState = 'paused'; run.pauseReason = 'recovery'; run.activeSince = undefined;
          }
        }
        this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(run), run.id);
      }
      this.db.exec('PRAGMA user_version=21;');
    });
    if (version < 22) {
      try { this.transaction(() => {
        for (const row of this.db.prepare('SELECT run_id, data FROM repository_work').all()) {
          const work = JSON.parse(String(row.data)) as RepositoryWorkRecord;
          const refreshes = work.refreshes ?? [];
          const interrupted = refreshes.filter(item => item.state === 'interrupted' && item.supersedes);
          if (!interrupted.length) continue;
          const byId = new Map(refreshes.map(item => [item.requestId, item]));
          if (byId.size !== refreshes.length) throw new Conflict('Interrupted retry history has duplicate request identities.');
          for (const retry of interrupted) {
            const prior = byId.get(retry.supersedes!.requestId);
            const next = retry.supersededBy && byId.get(retry.supersededBy.requestId);
            if (!prior || prior === retry || prior.state !== 'conflicted'
              || prior.supersededBy?.requestId !== retry.requestId || prior.supersededBy.reason !== retry.supersedes!.reason
              || prior.supersededBy.at !== retry.at || !retry.supersedes!.reason.trim()
              || (retry.supersededBy && (!next || next === retry || next.supersedes?.requestId !== retry.requestId
                || next.supersedes.reason !== retry.supersededBy.reason || next.at !== retry.supersededBy.at))
              || retry.resolutionWorkspace || retry.candidateId || retry.toHead || retry.conflicts?.length)
              throw new Conflict('Interrupted retry history is ambiguous; recover its persisted links and workspace before opening the workbench.');
            retry.state = 'conflicted'; retry.holdReason = 'interrupted_retry'; retry.conflicts = [];
          }
          if (refreshes.filter(item => item.state === 'conflicted' && !item.supersededBy).length > 1)
            throw new Conflict('Multiple active target holds need explicit recovery before opening the workbench.');
          this.db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(work), row.run_id);
          if (interrupted.some(item => !item.supersededBy)) {
            const run = this.getRun(work.runId);
            if (!['completed', 'closed', 'cancelled'].includes(run.status) && run.desiredState !== 'cancelled'
              && (run.desiredState !== 'paused' || !['paused', 'attention_required'].includes(run.status))) {
              this.changeRun(run.id, { type: 'desire', desiredState: 'paused' });
              this.changeRun(run.id, { type: 'attention', reason: 'An interrupted target retry requires an operator decision; reconcile process ownership before resuming.' });
            }
          }
        }
        this.db.exec('PRAGMA user_version=22;');
      }); } catch (error) { this.close(); throw error; }
    }
    if (version < 23) {
      try { this.transaction(() => {
        for (const row of this.db.prepare('SELECT run_id, data FROM repository_work').all()) {
          const work = JSON.parse(String(row.data)) as RepositoryWorkRecord;
          const record = this.codingRecord(work.runId);
          const checkedTerminal = (refresh: TargetRefresh) => {
            const workspace = refresh.resolutionWorkspace;
            const process = workspace?.promotionProcess;
            const completion = record?.completions?.find(item => item.id === workspace?.operationId);
            const rejection = completion?.rejection;
            const source = record?.candidates.find(item => item.id === refresh.sourceCandidateId);
            if (!workspace?.operationId || !workspace.toHead || !process || process.version !== 1
              || !completion || completion.conflictRefreshId !== refresh.requestId || completion.state !== 'rejected'
              || completion.candidateId || completion.head !== refresh.fromHead || !source
              || source.revision.base !== refresh.fromBase || source.revision.head !== refresh.fromHead
              || !record?.claims?.some(item => item.writerAttemptId === completion.attemptId && item.targetRefreshId === refresh.requestId)
              || !rejection || !['target_changed', 'promotion_interrupted'].includes(rejection.code)
              || rejection.expectedTarget !== refresh.toBase || !rejection.at || !rejection.reason
              || (rejection.code === 'target_changed' && (!rejection.observedTarget || rejection.observedTarget === refresh.toBase))
              || !Array.isArray(process.args) || !process.env || typeof process.env !== 'object'
              || !process.token || !process.epoch || !process.controllerRoot?.startsWith('/')
              || !process.command?.startsWith('/') || process.cwd !== work.path
              || !process.root?.endsWith(`/${completion.id}-${process.token}`)
              || workspace.branch !== `${work.branch}-refresh-${refresh.requestId}`
              || JSON.stringify(process.args.slice(-4)) !== JSON.stringify(['reset', '--keep', '--no-recurse-submodules', workspace.toHead])) return false;
            const raw = JSON.stringify({ version: 1, token: process.token, command: process.command, args: process.args,
              cwd: process.cwd, env: Object.fromEntries(Object.entries(process.env).sort(([a], [b]) => a.localeCompare(b))),
              epoch: process.epoch, controllerRoot: process.controllerRoot });
            if (createHash('sha256').update(raw).digest('hex') !== process.specSha256) return false;
            try {
              if (realpathSync(process.root) !== process.root) return false;
              const seal = JSON.parse(readFileSync(join(process.root, 'seal.json'), 'utf8'));
              if (seal.token !== process.token || seal.specSha256 !== process.specSha256 || Object.keys(seal).length !== 2) return false;
              const manifest = join(process.root, 'launch.json');
              return !existsSync(manifest) || createHash('sha256').update(readFileSync(manifest)).digest('hex') === process.specSha256;
            } catch { return false; }
          };
          let changed = false;
          for (const refresh of work.refreshes ?? []) {
            const workspace = refresh.resolutionWorkspace;
            if (refresh.state === 'conflicted' && workspace
              && (workspace.state === 'conflicted' || (workspace.state === 'operator_required' && !checkedTerminal(refresh)))) {
              // v22 could not prove that an in-flight add/rebase or unpromoted workspace child stopped.
              workspace.state = 'continuing'; workspace.legacyOwnershipUnknown = true; changed = true;
            }
          }
          if (changed) this.db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(work), row.run_id);
          const unknown = work.refreshes?.some(refresh => refresh.state === 'rebasing'
            || (refresh.state === 'conflicted' && ['preparing', 'continuing'].includes(refresh.resolutionWorkspace?.state ?? '')));
          if (unknown) {
            const run = this.getRun(work.runId);
            if (!['cancelled', 'completed', 'closed'].includes(run.status) && run.desiredState !== 'cancelled') {
              if (run.desiredState !== 'paused') this.changeRun(run.id, { type: 'desire', desiredState: 'paused' });
              this.changeRun(run.id, { type: 'attention', reason: 'Git mutation ownership from an older workbench is unverified; retain source and workspace for operator inspection.' });
            }
          }
        }
        this.db.exec('PRAGMA user_version=23;');
      }); } catch (error) { this.close(); throw error; }
    }
    if (version < 24) this.transaction(() => {
      // Old rows did not record terminal steering settlement or its time.
      for (const run of this.allRuns()) this.settleUndeliveredSteering(run);
      this.db.exec('PRAGMA user_version=24;');
    });
    if (version < 25) this.transaction(() => {
      this.db.exec(`ALTER TABLE team_proposals RENAME TO old_team_proposals;
        CREATE TABLE team_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
        INSERT INTO team_proposals SELECT * FROM old_team_proposals ORDER BY rowid;
        DROP TABLE old_team_proposals;
        CREATE UNIQUE INDEX operator_proposal_request ON team_proposals(run_id,tool_call_id) WHERE attempt_id IS NULL;
        PRAGMA user_version=25;`);
    });
    if (version < 26) this.transaction(() => {
      this.db.exec(`ALTER TABLE repository_work RENAME TO old_repository_work;
        CREATE TABLE repository_work (run_id TEXT PRIMARY KEY REFERENCES runs(id), common_dir TEXT NOT NULL, branch TEXT NOT NULL, data TEXT NOT NULL);
        INSERT INTO repository_work SELECT * FROM old_repository_work ORDER BY rowid;
        DROP TABLE old_repository_work;
        CREATE UNIQUE INDEX current_branch_owner ON repository_work(common_dir,branch) WHERE json_extract(data,'$.transferredTo') IS NULL;
        PRAGMA user_version=26;`);
    });
  }

  close() { if (!this.closed) { this.db.close(); this.closed = true; } }

  learningProposals(): LearningProposal[] {
    return this.db.prepare('SELECT data FROM learning_proposals ORDER BY rowid').all().map(row => JSON.parse(String(row.data)));
  }
  getLearningProposal(id: string): LearningProposal {
    const row = this.db.prepare('SELECT data FROM learning_proposals WHERE id=?').get(id);
    if (!row) throw new NotFound('Learning proposal not found.');
    return JSON.parse(String(row.data));
  }
  /** Idempotent per coordinator tool call; the same call with different content is rejected. */
  proposeLearning(input: Omit<LearningProposal, 'id' | 'status' | 'createdAt'>): LearningProposal {
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM learning_proposals WHERE attempt_id=? AND tool_call_id=?').get(input.attemptId, input.toolCallId);
      if (row) {
        const previous: LearningProposal = JSON.parse(String(row.data));
        const same = (value: Omit<LearningProposal, 'id' | 'status' | 'createdAt'>) => JSON.stringify([value.target, value.change, value.baseVersion, value.rationale, value.evidence]);
        if (same(previous) !== same(input)) throw new Conflict('Learning proposal identity has different content.');
        return previous;
      }
      const proposal: LearningProposal = { ...structuredClone(input), id: randomUUID(), status: 'pending', createdAt: new Date(this.clock()).toISOString() };
      this.db.prepare('INSERT INTO learning_proposals VALUES(?,?,?,?,?)').run(proposal.id, proposal.runId, proposal.attemptId, proposal.toolCallId, JSON.stringify(proposal));
      return proposal;
    });
  }
  /**
   * Record the operator's decision. Project guidance is applied in the same transaction, against the version
   * the proposal was based on. An approved role change is recorded with `pendingRoleSave` before the catalog
   * is saved, and completed by `completeLearningApproval`.
   */
  decideLearning(id: string, decision: { requestId: string; reason: string; status: 'approved' | 'rejected'; pendingRoleSave?: boolean }): LearningProposal {
    return this.transaction(() => {
      const proposal = this.getLearningProposal(id);
      if (proposal.status !== 'pending') {
        if (proposal.status === decision.status && proposal.decision?.requestId === decision.requestId) return proposal;
        throw new Conflict('This proposal has already been decided.');
      }
      proposal.status = decision.status;
      proposal.decision = { requestId: decision.requestId, at: new Date(this.clock()).toISOString(), reason: decision.reason, ...(decision.pendingRoleSave ? { pendingRoleSave: true } : {}) };
      if (decision.status === 'approved' && proposal.target.kind === 'project') {
        const repositoryId = proposal.target.repositoryId;
        const repository = this.snapshot().repositories.find(item => item.id === repositoryId);
        if (!repository) throw new NotFound('Repository not found.');
        const current = repository.guidance?.at(-1)?.version ?? 0;
        if (current !== proposal.baseVersion) throw new Conflict('Project guidance changed since this proposal. Ask the coordinator for a new proposal.');
        const version = current + 1;
        repository.guidance = [...(repository.guidance ?? []), { version, text: proposal.change, approvedAt: proposal.decision.at, proposalId: proposal.id }];
        this.db.prepare('UPDATE repositories SET data=? WHERE id=?').run(JSON.stringify(repository), repository.id);
        proposal.decision.appliedVersion = version;
      }
      this.db.prepare('UPDATE learning_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), id);
      return proposal;
    });
  }
  /** Finish an approved role change: its saved revision, or why nothing was applied. */
  completeLearningApproval(id: string, outcome: { appliedVersion: number } | { notApplied: string }): LearningProposal {
    return this.transaction(() => {
      const proposal = this.getLearningProposal(id);
      if (!proposal.decision?.pendingRoleSave) return proposal;
      delete proposal.decision.pendingRoleSave;
      if ('appliedVersion' in outcome) proposal.decision.appliedVersion = outcome.appliedVersion;
      else { proposal.status = 'superseded'; proposal.decision.notApplied = outcome.notApplied; }
      this.db.prepare('UPDATE learning_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), id);
      return proposal;
    });
  }
  /** The latest approved guidance for the repository a run targets, snapshotted into each new assignment. */
  private currentGuidance(runId: string) {
    const repositoryId = this.getRun(runId).taskSnapshot.repositoryId;
    const row = repositoryId ? this.db.prepare('SELECT data FROM repositories WHERE id=?').get(repositoryId) : undefined;
    const latest = row ? (JSON.parse(String(row.data)) as Repository).guidance?.at(-1) : undefined;
    return latest && { version: latest.version, text: latest.text };
  }

  deliveryNotices(runId: string): DeliveryNotice[] {
    return this.db.prepare('SELECT data FROM delivery_notices WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)));
  }
  noticeDelivery(runId: string, payload: DeliveryNotice['payload']) {
    return this.transaction(() => {
      this.getRun(runId);
      const previous = this.deliveryNotices(runId).findLast(item => item.payload.kind === payload.kind && item.payload.subjectId === payload.subjectId);
      if (previous && JSON.stringify(previous.payload) === JSON.stringify(payload)) return previous;
      const notice: DeliveryNotice = { id: randomUUID(), runId, payload, createdAt: new Date().toISOString(), disposition: 'pending' };
      this.db.prepare('INSERT INTO delivery_notices VALUES(?,?,?)').run(notice.id, runId, JSON.stringify(notice)); return notice;
    });
  }
  dispatchDeliveryNotice(attemptId: string, id: string, received = false) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId); const run = this.getRun(attempt.runId);
      const notice = this.deliveryNotices(run.id).find(item => item.id === id);
      if (!this.runtime(run.id).assignments.some(item => item.coordinator && !item.retiredAt && item.currentAttemptId === attemptId)) throw new Conflict('Delivery notices target only the current coordinator.');
      if (!notice || (received ? notice.disposition !== 'dispatched' || notice.attemptId !== attemptId : notice.disposition !== 'pending' || attempt.ownership !== 'held' || run.status !== 'running' || run.desiredState !== 'running')) throw new Conflict('Delivery notice is not available for this attempt.');
      notice.disposition = received ? 'received' : 'dispatched'; notice.attemptId = attemptId;
      this.db.prepare('UPDATE delivery_notices SET data=? WHERE id=?').run(JSON.stringify(notice), id); return notice;
    });
  }

  publications(): PublicationRecord[] { return this.db.prepare('SELECT data FROM publications ORDER BY rowid').all().map(row => JSON.parse(String(row.data))); }
  publication(runId: string): PublicationRecord | undefined { const row = this.db.prepare('SELECT data FROM publications WHERE run_id=?').get(runId); return row ? JSON.parse(String(row.data)) : undefined; }
  savePublication(record: PublicationRecord) {
    return this.transaction(() => {
      const current = this.publication(record.runId);
      if (current?.id === record.id && current.feedback && (!record.feedback || current.feedback.observedAt >= record.feedback.observedAt)) record.feedback = current.feedback;
      this.db.prepare('INSERT INTO publications VALUES(?,?) ON CONFLICT(run_id) DO UPDATE SET data=excluded.data').run(record.runId, JSON.stringify(record));
      if (record.merged) this.releaseCodingSlotInTransaction(record.runId, 'merged');
      this.accountTimeInTransaction(record.runId);
      return record;
    });
  }
  savePublicationFeedback(runId: string, prId: number, read: BitbucketCommentRead, observedAt: string): FeedbackObservation {
    return this.transaction(() => {
      const record = this.publication(runId);
      if (!record || record.pr.id !== prId || record.pr.state !== 'confirmed') throw new Conflict('The observed PR no longer matches this publication.');
      const previous = record.feedback;
      if (previous && previous.observedAt > observedAt) return previous;
      const known = new Map((previous?.comments ?? []).map(comment => [comment.id, comment]));
      let inconsistent = false;
      for (const comment of read.comments) {
        const prior = known.get(comment.id);
        if (!prior) { known.set(comment.id, { ...comment, currentness: 'known' }); continue; }
        const earlier = Date.parse(comment.updated_on) < Date.parse(prior.updated_on);
        const same = Date.parse(comment.updated_on) === Date.parse(prior.updated_on);
        const conflict = same && (prior.content?.raw !== comment.content?.raw || prior.deleted !== comment.deleted);
        if (earlier || conflict) {
          known.set(comment.id, { ...prior, currentness: conflict ? 'conflict' : 'unknown' }); inconsistent = true;
        } else known.set(comment.id, { ...comment, currentness: 'known' });
      }
      for (const [id, comment] of known) if (!read.comments.some(item => item.id === id)) known.set(id, { ...comment, currentness: 'unknown' });
      const feedback: FeedbackObservation = { observedAt, state: inconsistent ? 'partial' : read.state,
        reason: inconsistent ? 'Bitbucket returned an older or conflicting comment version; prior content was retained.' : read.reason,
        comments: [...known.values()].sort((a, b) => Date.parse(a.created_on) - Date.parse(b.created_on) || a.id - b.id) };
      record.feedback = feedback;
      this.db.prepare('UPDATE publications SET data=? WHERE run_id=?').run(JSON.stringify(record), runId);
      return feedback;
    });
  }

  jiraUpdates(runId?: string): JiraUpdate[] {
    const rows = runId ? this.db.prepare('SELECT data FROM jira_updates WHERE run_id=? ORDER BY rowid').all(runId) : this.db.prepare('SELECT data FROM jira_updates ORDER BY rowid').all();
    return rows.map(row => JSON.parse(String(row.data)));
  }
  saveJiraUpdate(update: JiraUpdate) {
    this.db.prepare('INSERT INTO jira_updates VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data').run(update.id, update.runId, update.eventKey, JSON.stringify(update));
    if (['confirmed', 'uncertain', 'blocked'].includes(update.state)) this.noticeDelivery(update.runId, { kind: 'jira', subjectId: update.id, eventKey: update.eventKey, state: update.state, externalId: update.externalId });
    return update;
  }

  getRun(id: string): Run {
    const row = this.db.prepare('SELECT data FROM runs WHERE id=?').get(id);
    if (!row) throw new NotFound('Run not found.');
    return JSON.parse(String(row.data));
  }

  codingRecord(runId: string): CodingRecord | undefined {
    this.getRun(runId); const row = this.db.prepare('SELECT data FROM coding WHERE run_id=?').get(runId);
    return row ? JSON.parse(String(row.data)) : undefined;
  }
  updateCodingRecord(runId: string, update: (record: CodingRecord | undefined) => CodingRecord) {
    return this.transaction(() => {
      const record = update(this.codingRecord(runId));
      if (record.runId !== runId) throw new Conflict('Coding record belongs to another run.');
      this.db.prepare('INSERT INTO coding VALUES(?,?) ON CONFLICT(run_id) DO UPDATE SET data=excluded.data').run(runId, JSON.stringify(record)); return record;
    });
  }

  repositoryWork(runId: string): RepositoryWorkRecord | undefined {
    this.getRun(runId);
    const row = this.db.prepare('SELECT data FROM repository_work WHERE run_id=?').get(runId);
    return row ? JSON.parse(String(row.data)) : undefined;
  }

  reserveRepositoryWork(record: RepositoryWorkRecord) {
    return this.transaction(() => {
      const run = this.getRun(record.runId);
      if ((run.purpose ?? 'coding') !== 'coding' || run.taskSnapshot.repositoryId !== record.repository.id || run.status !== 'awaiting_runner' || ['cancelled', 'paused'].includes(run.desiredState ?? '') || this.runtime(run.id).attempts.some(attempt => attempt.state !== 'admitted')) throw new Conflict('Only a coding run can prepare repository ownership before execution.');
      const selected = this.repositoryById(record.repository.id);
      if (selected.retiredAt || (selected.revision ?? 1) !== (record.repository.revision ?? 1))
        throw new RepositoryConfigurationChanged('Repository configuration changed during preparation; retry from current configuration.');
      if (this.repositoryWork(run.id)) throw new Conflict('Repository work is already reserved.');
      if (this.db.prepare("SELECT run_id FROM repository_work WHERE common_dir=? AND branch=? AND json_extract(data,'$.transferredTo') IS NULL").get(record.commonDir, record.branch)) throw new Conflict('The branch is already owned by another run.');
      this.db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(record.runId, record.commonDir, record.branch, JSON.stringify(record));
      return record;
    });
  }

  confirmRepositoryWork(runId: string, gitFile: string, instructions: RepositoryWorkRecord['instructions'], checkpoint: RepositoryCheckpoint) {
    return this.transaction(() => {
      const work = this.repositoryWork(runId);
      if (!work) throw new NotFound('Repository work is not reserved.');
      if (work.state === 'ready') return work;
      work.state = 'ready'; work.gitFile = gitFile; work.instructions = instructions; work.checkpoints.push(checkpoint);
      return this.saveRepositoryWork(work);
    });
  }

  claimRepositoryWriter(attemptId: string, checkpoint: RepositoryCheckpoint, claim?: ImplementationClaim) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId); const run = this.getRun(attempt.runId);
      const work = this.repositoryWork(run.id);
      if (work?.refreshes?.some(item => item.state === 'rebasing'
        || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
        throw new Conflict('Unverified Git mutation ownership blocks repository writers.');
      const unresolved = work?.refreshes?.findLast(item => item.state === 'rebasing' || (item.state === 'conflicted' && !item.supersededBy));
      const conflict = unresolved?.state === 'conflicted' && ['conflicted', 'ready'].includes(unresolved.resolutionWorkspace?.state ?? '') ? unresolved : undefined;
      if (unresolved && (!conflict || claim?.targetRefreshId !== unresolved.requestId)) throw new Conflict('Target conflict requires an explicitly scoped writer and approved workspace.');
      if (claim?.targetRefreshId && claim.targetRefreshId !== conflict?.requestId) throw new Conflict('Implementation claim targets a different conflict workspace.');
      const publication = this.publication(run.id);
      const feedback = run.feedbackContinuations?.find(item => item.id === claim?.feedbackContinuationId);
      const coding = this.codingRecord(run.id);
      const source = coding?.candidates.at(-1);
      const priorGrant = claim?.restoredFrom && coding?.claims?.findLast(item => item.writerAttemptId === claim.restoredFrom && item.feedbackContinuationId === feedback?.id);
      const priorAttempt = claim?.restoredFrom && priorGrant ? this.getAttempt(claim.restoredFrom) : undefined;
      const checkpointBeforeRestore = work?.checkpoints.at(-1);
      const restored = !!priorGrant && !!priorAttempt && attempt.previousAttemptId === priorAttempt.id && priorAttempt.assignmentId === attempt.assignmentId
        && priorAttempt.ownership === 'stopped' && priorGrant.candidateId === source?.id && priorGrant.instruction === claim?.instruction
        && checkpointBeforeRestore?.reason === 'interrupted' && JSON.stringify(checkpointBeforeRestore.revision) === JSON.stringify(checkpoint.revision)
        && checkpoint.revision.head === source?.revision.head && checkpoint.revision.tree === source.revision.tree
        && checkpoint.revision.base === source.revision.base && checkpoint.revision.requirementsHash === source.revision.requirementsHash
        && checkpoint.revision.instructionsHash === source.revision.instructionsHash;
      const rootCandidate = coding?.candidates.find(item => item.id === feedback?.sourceCandidateId);
      const activeFeedback = !!source && !!feedback && (['starting', 'handed_off'].includes(feedback.state)
        && source.id === feedback.sourceCandidateId || feedback.state === 'candidate' && feedback.candidateId === source.id
          && source.feedbackFrom?.continuationId === feedback.id);
      const feedbackAllowed = !!publication && !!feedback && activeFeedback
        && publication.id === feedback.publicationId && publication.pr.id === feedback.prId && publication.repository.uuid === feedback.repository.uuid
        && publication.pr.state === 'confirmed' && publication.pr.providerState === 'OPEN' && publication.push.state === 'confirmed'
        && publication.candidateId === rootCandidate?.id && rootCandidate?.revision.head === feedback.sourceHead
        && (claim?.restoredFrom ? restored : JSON.stringify(checkpoint.revision) === JSON.stringify(source?.revision))
        && publication.branch === work?.branch && publication.repositoryPath === work?.repository.path;
      if (publication && (!conflict && !feedbackAllowed || publication.merged
        || ['sending', 'uncertain', 'blocked'].includes(publication.push.state) || ['sending', 'uncertain', 'blocked'].includes(publication.pr.state)))
        throw new Conflict('Publication owns this revision. Explicit review continuation is required before more repository writes.');
      const assignment = this.runtime(run.id).assignments.find(item => item.currentAttemptId === attemptId);
      if (!work || work.state !== 'ready' || !assignment || assignment.retiredAt || assignment.coordinator || attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running') throw new Conflict('Only an active specialist may own repository writes.');
      if (assignment.rebriefRequired) throw new Conflict('This assignment needs a new brief after requirements adoption.');
      if (this.teamProposals(run.id).some(item => item.status === 'pending' && item.retirements?.some(target => target.assignmentId === assignment.id))) throw new Conflict('This specialist is pending retirement.');
      if (work.writer?.attemptId === attemptId) return work;
      // One writer per worktree. Each run owns its own worktree.
      if (work.writer) throw new Conflict('Another writer owns repository work. Confirm termination before replacement.');
      if (claim) {
        const coding = this.codingRecord(run.id);
        if (!coding || claim.writerAttemptId !== attemptId) throw new Conflict('Implementation claim is not configured.');
        coding.repairAssignmentId ??= assignment.id;
        const owner = this.runtime(run.id).assignments.find(item => item.id === coding.repairAssignmentId)!;
        if (claim.automatic) {
          if ((owner.automaticCycles ?? 0) >= automaticCycleLimit(owner)) throw new Conflict(cyclesExhausted(owner, 'repair/replacement'));
          owner.automaticCycles = (owner.automaticCycles ?? 0) + 1;
          this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(owner), owner.id);
        }
        coding.claims = [...(coding.claims ?? []), claim];
        this.db.prepare('UPDATE coding SET data=? WHERE run_id=?').run(JSON.stringify(coding), run.id);
      }
      work.writer = { attemptId, acquiredAt: new Date().toISOString() }; work.checkpoints.push(checkpoint);
      return this.saveRepositoryWork(work);
    });
  }

  releaseRepositoryWriter(attemptId: string, checkpoint: RepositoryCheckpoint) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId); const work = this.repositoryWork(attempt.runId);
      if (!work?.writer || work.writer.attemptId !== attemptId) throw new Conflict('Attempt does not own repository writes.');
      if (attempt.ownership !== 'stopped') throw new Conflict('Confirm stopped ownership before releasing the writer.');
      work.writer = undefined; work.checkpoints.push(checkpoint); return this.saveRepositoryWork(work);
    });
  }

  checkpointRepository(runId: string, checkpoint: RepositoryCheckpoint) {
    return this.transaction(() => {
      const work = this.repositoryWork(runId);
      if (!work || work.writer || this.runtime(runId).attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped')) throw new Conflict('Confirm stopped ownership before checkpointing a completed revision.');
      work.checkpoints.push(checkpoint); return this.saveRepositoryWork(work);
    });
  }

  /** Reject stale or busy refresh requests before transport work; the transaction checks again. */
  preflightTargetRefresh(runId: string, supersedes?: TargetRefresh['supersedes']) {
    const run = this.getRun(runId); const work = this.repositoryWork(runId);
    if (!work || work.state !== 'ready') throw new NotFound('Repository work has not been prepared.');
    if (!['paused', 'attention_required'].includes(run.status) || work.writer || this.runtime(runId).attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped')) throw new Conflict('Pause the run and confirm every agent stopped before refreshing from the target.');
    if (this.codingRecord(runId)?.completions?.some(item => item.state === 'requested')) throw new Conflict('Reconcile pending implementation completion before refreshing from the target.');
    if (work.refreshes?.some(item => ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
      throw new Conflict('Unverified Git mutation ownership blocks target refresh.');
    const unresolved = work.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
    if (unresolved) {
      if (!supersedes || supersedes.requestId !== unresolved.requestId || !supersedes.reason.trim()
        || (unresolved.resolutionWorkspace && unresolved.resolutionWorkspace.state !== 'operator_required')) throw new Conflict('An operator reason is required to supersede this held target conflict.');
    } else if (supersedes) throw new Conflict('The operator retry no longer matches an active target hold.');
    if (work.refreshes?.some(item => item.state === 'rebasing')) throw new Conflict('Reconcile the current target refresh before starting another.');
    const publication = this.publication(runId);
    if (publication && (['sending', 'uncertain', 'blocked'].includes(publication.push.state) || ['sending', 'uncertain', 'blocked'].includes(publication.pr.state))) throw new Conflict('Reconcile the publication receipts before refreshing from the target.');
    return work;
  }

  /**
   * Atomically record refresh intent or a verified unchanged-target decision with supersession of its named hold.
   * Both require a stopped, non-executing run and reconciled publication writes; this does not grant readiness.
   */
  beginTargetRefresh(runId: string, refresh: TargetRefresh) {
    return this.transaction(() => {
      const work = this.preflightTargetRefresh(runId, refresh.supersedes);
      const source = this.codingRecord(runId)?.candidates.at(-1);
      if (!refresh.sourceCandidateId || !source || source.id !== refresh.sourceCandidateId || source.revision.head !== refresh.fromHead
        || source.revision.base !== refresh.fromBase || work.base !== refresh.fromBase)
        throw new Conflict('The target refresh source candidate changed before admission.');
      if (refresh.state === 'completed' && (refresh.toBase !== refresh.fromBase || refresh.toHead !== refresh.fromHead
        || refresh.candidateId !== source.id)) throw new Conflict('An unchanged target receipt must already identify its source candidate.');
      if (work.refreshes?.some(item => item.requestId === refresh.requestId)) throw new Conflict('This target refresh request is already recorded.');
      const unresolved = work.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
      if (unresolved) unresolved.supersededBy = { requestId: refresh.requestId, reason: refresh.supersedes!.reason, at: refresh.at };
      if (refresh.state === 'completed' && work.continuationTargetRef) { work.targetRef = refresh.targetRef; work.continuationTargetRef = undefined; }
      work.refreshes = [...(work.refreshes ?? []), refresh]; return this.saveRepositoryWork(work);
    });
  }

  /** Persist isolated worktree ownership and its monotonic conflict-resolution state. */
  recordTargetRefreshWorkspace(runId: string, requestId: string, workspace: TargetRefreshWorkspace, conflicts?: string[]) {
    return this.transaction(() => {
      const work = this.repositoryWork(runId); const refresh = work?.refreshes?.find(item => item.requestId === requestId);
      if (!work || !refresh || !['rebasing', 'conflicted'].includes(refresh.state)) throw new Conflict('Target refresh is not preparing or resolving a conflict workspace.');
      const previous = refresh.resolutionWorkspace;
      if (previous && (previous.path !== workspace.path || previous.branch !== workspace.branch)) throw new Conflict('Target refresh workspace identity changed.');
      if (previous && (previous.requirementsHash !== workspace.requirementsHash || previous.instructionsHash !== workspace.instructionsHash)) throw new Conflict('Conflict workspace accepted-scope snapshot changed.');
      if (previous && ['ready', 'promoting', 'promoted'].includes(previous.state)
        && (previous.toHead !== workspace.toHead || previous.operationId !== workspace.operationId || JSON.stringify(previous.promotionProcess) !== JSON.stringify(workspace.promotionProcess))) throw new Conflict('Conflict promotion identity changed.');
      const next: Record<TargetRefreshWorkspace['state'], TargetRefreshWorkspace['state'][]> = {
        preparing: ['preparing', 'conflicted', 'operator_required'], conflicted: ['conflicted', 'ready', 'operator_required'],
        continuing: ['continuing'], operator_required: ['operator_required'], ready: ['ready', 'operator_required', 'promoting'], promoting: ['promoting'], promoted: ['promoted'],
      };
      if (previous && !next[previous.state].includes(workspace.state)) throw new Conflict('Target refresh workspace state cannot move backward.');
      if (previous && (JSON.stringify(previous.continuation) !== JSON.stringify(workspace.continuation)
        || JSON.stringify(previous.continuationReceipts) !== JSON.stringify(workspace.continuationReceipts))) throw new Conflict('Conflict continuation identity cannot change through workspace updates.');
      if (conflicts) refresh.conflicts = conflicts;
      refresh.resolutionWorkspace = workspace; return this.saveRepositoryWork(work);
    });
  }

  /** Save the exact writer, workspace and paths before the first isolated Git mutation. */
  beginTargetConflictContinuation(runId: string, requestId: string, identity: TargetConflictContinuation) {
    return this.transaction(() => {
      const run = this.getRun(runId); const work = this.repositoryWork(runId);
      const refresh = work?.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
      const workspace = refresh?.resolutionWorkspace;
      const attempt = this.getAttempt(identity.attemptId);
      if (!work || !refresh || refresh.requestId !== requestId || workspace?.state !== 'conflicted' || workspace.continuation
        || identity.requestId !== requestId || identity.path !== workspace.path || identity.branch !== workspace.branch
        || identity.sourceHead !== refresh.fromHead || !identity.paths.length || identity.paths.some(path => !refresh.conflicts?.includes(path))
        || workspace.continuationReceipts?.some(item => item.attemptId === identity.attemptId && item.toolCallId === identity.toolCallId)
        || work.writer?.attemptId !== identity.attemptId || attempt.runId !== runId || attempt.ownership !== 'held'
        || !this.runtime(runId).assignments.some(item => item.currentAttemptId === identity.attemptId)
        || run.status !== 'running' || run.desiredState !== 'running')
        throw new Conflict('The scoped conflict writer or Git continuation changed before admission.');
      workspace.state = 'continuing'; workspace.continuation = identity;
      return this.saveRepositoryWork(work);
    });
  }

  /** Settle only the matching returned and validated operation, preserving its replay receipt. */
  finishTargetConflictContinuation(runId: string, requestId: string, identity: TargetConflictContinuation, result: TargetConflictContinuationResult) {
    return this.transaction(() => {
      const run = this.getRun(runId); const work = this.repositoryWork(runId);
      const refresh = work?.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
      const workspace = refresh?.resolutionWorkspace;
      if (!work || !refresh || refresh.requestId !== requestId || workspace?.state !== 'continuing'
        || JSON.stringify(workspace.continuation) !== JSON.stringify(identity)
        || work.writer?.attemptId !== identity.attemptId || this.getAttempt(identity.attemptId).ownership !== 'held'
        || run.status !== 'running' || run.desiredState !== 'running' || (result.state === 'ready' && !result.head))
        throw new Conflict('Git continuation cannot settle without its current writer and validated identity.');
      workspace.state = result.state; workspace.toHead = result.head ?? workspace.toHead;
      workspace.continuation = undefined;
      workspace.continuationReceipts = [...(workspace.continuationReceipts ?? []), { ...identity, result }];
      refresh.conflicts = result.conflicts;
      return this.saveRepositoryWork(work);
    });
  }

  beginTargetPromotion(runId: string, requestId: string, operationId: string, toHead: string, process: NonNullable<TargetRefreshWorkspace['promotionProcess']>) {
    return this.transaction(() => {
      const run = this.getRun(runId); const work = this.repositoryWork(runId);
      const refresh = work?.refreshes?.find(item => item.requestId === requestId); const workspace = refresh?.resolutionWorkspace;
      if (!work || !refresh || refresh.state !== 'conflicted' || !workspace || !['ready', 'promoting'].includes(workspace.state)) throw new Conflict('A ready isolated target conflict is required for promotion.');
      if (workspace.state === 'promoting' && (workspace.operationId !== operationId || workspace.toHead !== toHead
        || JSON.stringify(workspace.promotionProcess) !== JSON.stringify(process))) throw new Conflict('Target conflict promotion identity changed.');
      if (process.version !== 1 || !process.root.endsWith(`${operationId}-${process.token}`) || process.cwd !== work.path || process.args.at(-1) !== toHead) throw new Conflict('Promotion process does not match the recorded source.');
      if (run.status !== 'paused' || run.desiredState !== 'running' || work.writer || this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Confirm all agent ownership stopped before promoting a conflict.');
      if (!this.codingRecord(runId)?.completions?.some(item => item.id === operationId && item.state === 'requested' && item.conflictRefreshId === requestId)) throw new Conflict('A recorded conflict completion is required.');
      if (workspace.state === 'ready') Object.assign(workspace, { state: 'promoting', operationId, toHead, promotionProcess: process });
      return this.saveRepositoryWork(work);
    });
  }
  /** Terminal outcomes share the same stopped, clean, source-scoped proof and atomic retained-workspace transition. */
  rejectUnpromotedConflict(runId: string, requestId: string, operationId: string, proof: UnpromotedConflictProof & { observedTarget: string }) {
    return this.terminalUnpromotedConflict(runId, requestId, operationId, proof, { code: 'target_changed', observedTarget: proof.observedTarget });
  }
  interruptUnpromotedConflict(runId: string, requestId: string, operationId: string, proof: UnpromotedConflictProof) {
    return this.terminalUnpromotedConflict(runId, requestId, operationId, proof, { code: 'promotion_interrupted' });
  }
  private terminalUnpromotedConflict(runId: string, requestId: string, operationId: string, proof: UnpromotedConflictProof,
    outcome: { code: 'target_changed'; observedTarget: string } | { code: 'promotion_interrupted' }) {
    return this.transaction(() => {
      const run = this.getRun(runId); const work = this.repositoryWork(runId);
      const refresh = work?.refreshes?.find(item => item.requestId === requestId);
      const workspace = refresh?.resolutionWorkspace; const record = this.codingRecord(runId);
      const completion = record?.completions?.find(item => item.id === operationId);
      const source = record?.candidates.at(-1);
      const owner = completion && this.getAttempt(completion.attemptId);
      if (!work || !refresh || refresh.state !== 'conflicted' || refresh.supersededBy
        || work.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy)?.requestId !== requestId
        || !workspace || !['ready', 'promoting'].includes(workspace.state)
        || (workspace.state === 'promoting' && workspace.operationId !== operationId)
        || !workspace.gitFile || !workspace.toHead || workspace.toHead !== proof.toHead
        || workspace.gitFile !== proof.workspaceGitFile || work.gitFile !== proof.originalGitFile
        || workspace.path !== proof.workspacePath || workspace.branch !== proof.workspaceBranch
        || (workspace.requirementsHash ?? work.requirementsHash) !== proof.requirementsHash
        || (workspace.instructionsHash ?? createHash('sha256').update(JSON.stringify(work.instructions)).digest('hex')) !== proof.instructionsHash
        || !completion || completion.state !== 'requested' || completion.conflictRefreshId !== requestId
        || owner?.runId !== runId || owner.ownership !== 'stopped'
        || !record?.claims?.some(item => item.writerAttemptId === completion.attemptId && item.targetRefreshId === requestId)
        || completion.candidateId || completion.head !== proof.originalHead || completion.worktreeHash !== proof.worktreeHash
        || work.path !== proof.originalPath || work.branch !== proof.originalBranch || work.base !== proof.originalBase
        || work.targetRef !== proof.originalTargetRef || !refresh.sourceCandidateId || source?.id !== refresh.sourceCandidateId
        || source.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase
        || refresh.fromHead !== proof.originalHead || refresh.fromBase !== proof.originalBase || refresh.targetRef !== proof.targetRef
        || refresh.toBase !== proof.expectedTarget || (outcome.code === 'target_changed' && (!outcome.observedTarget || outcome.observedTarget === proof.expectedTarget))
        || work.writer || !['paused', 'attention_required'].includes(run.status) || run.desiredState === 'cancelled'
        || this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped'))
        throw new Conflict('Unpromoted conflict termination lacks stopped, unmoved proof.');
      const at = new Date().toISOString();
      const rejection = outcome.code === 'target_changed'
        ? { code: 'target_changed' as const, at, expectedTarget: proof.expectedTarget, observedTarget: outcome.observedTarget,
          reason: 'The live target moved before the resolved conflict could be promoted.' }
        : { code: 'promotion_interrupted' as const, at, expectedTarget: proof.expectedTarget,
          reason: 'The checked Git reset stopped without applying the resolved conflict; an explicit operator retry is required.' };
      completion.state = 'rejected'; completion.rejection = rejection;
      workspace.state = 'operator_required';
      if (run.codingTransition?.completionId === operationId) { run.codingTransition = undefined; this.saveRun(run); }
      this.db.prepare('UPDATE coding SET data=? WHERE run_id=?').run(JSON.stringify(record), runId);
      this.saveRepositoryWork(work);
      const payload: DeliveryNotice['payload'] = { kind: outcome.code === 'target_changed' ? 'target_conflict_rejected' : 'target_conflict_interrupted',
        subjectId: operationId, ...rejection, refreshId: requestId, instruction: outcome.code === 'target_changed'
          ? 'The live target moved before conflict promotion. The isolated workspace is retained; choose a reasoned retry from the current target. The original currently matches its recorded clean source; no candidate or Crewroom PR write was recorded.'
          : 'The checked Git reset did not apply. The original currently matches its recorded clean source, and the resolved isolated workspace remains. Choose an explicit reasoned retry even if the target has not changed; do not replay the rejected completion.' };
      if (!this.deliveryNotices(runId).some(item => item.payload.kind === payload.kind && item.payload.subjectId === operationId)) {
        const notice: DeliveryNotice = { id: randomUUID(), runId, payload, createdAt: rejection.at, disposition: 'pending' };
        this.db.prepare('INSERT INTO delivery_notices VALUES(?,?,?)').run(notice.id, runId, JSON.stringify(notice));
      }
      return completion;
    });
  }

  /** Only the caller that observed its awaited Git child exit records a stopped rebase; rollback is fenced separately. */
  recordTargetRebaseRecovery(runId: string, requestId: string, stage: NonNullable<TargetRefresh['rebaseRecovery']>) {
    return this.transaction(() => {
      const work = this.repositoryWork(runId); const refresh = work?.refreshes?.find(item => item.requestId === requestId);
      if (!work || !refresh || refresh.state !== 'rebasing' || refresh.resolutionWorkspace
        || (stage === 'rebase_stopped' ? !!refresh.rebaseRecovery
          : stage === 'undo_started' ? refresh.rebaseRecovery !== 'rebase_stopped'
            : refresh.rebaseRecovery !== 'undo_started'))
        throw new Conflict('Target rebase recovery stage changed or lacks an observed stopped child.');
      refresh.rebaseRecovery = stage;
      return this.saveRepositoryWork(work);
    });
  }

  /** Record a Git refresh outcome and checkpoint, or associate the existing candidate with a completed unchanged-target receipt. */
  finishTargetRefresh(runId: string, requestId: string, outcome: Pick<TargetRefresh, 'state' | 'toHead' | 'conflicts' | 'candidateId' | 'resolutionWorkspace'>, checkpoint?: RepositoryCheckpoint) {
    return this.transaction(() => {
      const work = this.repositoryWork(runId); const refresh = work?.refreshes?.find(item => item.requestId === requestId);
      if (!work || !refresh) throw new NotFound('Target refresh is not recorded.');
      const promoting = refresh.state === 'conflicted' && refresh.resolutionWorkspace?.state === 'promoting' && outcome.state === 'completed';
      if (refresh.state === 'rebasing' || promoting) {
        if (outcome.state === 'completed') {
          const source = this.codingRecord(runId)?.candidates.at(-1);
          const run = this.getRun(runId);
          if (!checkpoint || (!promoting && refresh.rebaseRecovery !== 'rebase_stopped') || (promoting && (refresh.resolutionWorkspace?.toHead !== outcome.toHead
            || !this.codingRecord(runId)?.completions?.some(item => item.id === refresh.resolutionWorkspace?.operationId && item.state === 'requested' && item.conflictRefreshId === requestId)))
            || checkpoint.revision.base !== refresh.toBase || checkpoint.revision.head !== outcome.toHead || checkpoint.revision.dirty)
            throw new Conflict('A completed refresh needs its matching clean checkpoint.');
          if (work.base !== refresh.fromBase || work.writer || (promoting ? !['paused', 'attention_required'].includes(run.status) : run.status !== 'paused')
            || (promoting && run.desiredState === 'cancelled') || this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')
            || (refresh.sourceCandidateId && source?.id !== refresh.sourceCandidateId)
            || source?.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase
            || (!promoting && this.codingRecord(runId)?.completions?.some(item => item.state === 'requested')))
            throw new Conflict('Refresh source or stopped ownership changed before completion.');
          work.base = refresh.toBase; work.targetRef = refresh.targetRef; work.continuationTargetRef = undefined; work.checkpoints.push(checkpoint);
        }
        if (outcome.state === 'interrupted' && refresh.supersedes) {
          const source = this.codingRecord(runId)?.candidates.at(-1);
          const run = this.getRun(runId);
          if (checkpoint || outcome.toHead || outcome.candidateId || outcome.resolutionWorkspace || refresh.resolutionWorkspace
            || refresh.rebaseRecovery !== 'restored' || refresh.toHead || refresh.candidateId || work.base !== refresh.fromBase || work.writer || run.status !== 'paused'
            || this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')
            || this.codingRecord(runId)?.completions?.some(item => item.state === 'requested')
            || !source || source.id !== refresh.sourceCandidateId || source.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase)
            throw new Conflict('Interrupted retry lacks stopped, unchanged source proof.');
          Object.assign(refresh, { state: 'conflicted', holdReason: 'interrupted_retry', conflicts: [] });
          return this.saveRepositoryWork(work);
        }
        if (outcome.state === 'interrupted' && !refresh.supersedes && refresh.rebaseRecovery !== 'restored')
          throw new Conflict('An interrupted refresh requires a completed, clean rollback receipt.');
        Object.assign(refresh, outcome);
      } else if (outcome.candidateId && refresh.state === 'completed') refresh.candidateId ??= outcome.candidateId;
      return this.saveRepositoryWork(work);
    });
  }

  private saveRepositoryWork(work: RepositoryWorkRecord) {
    this.db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(work), work.runId); return work;
  }

  runtime(runId: string) {
    this.getRun(runId);
    return {
      assignments: this.db.prepare('SELECT data FROM assignments WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)) as Assignment),
      attempts: this.db.prepare('SELECT a.data FROM attempts a JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? ORDER BY a.rowid').all(runId).map(row => JSON.parse(String(row.data)) as Attempt),
      messages: this.db.prepare('SELECT data FROM peer_messages WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)) as PeerMessage),
      deliveryNotices: this.deliveryNotices(runId),
    };
  }

  /** A brief without obligations or dependencies is valid; the supervisor snapshots validated briefs for every admitted team. */
  admitAssignment(runId: string, input: { name: string; role: unknown; model: AssignmentModel; coordinator: boolean; input: string; brief?: AssignmentBrief }) {
    return this.transaction(() => this.admitAssignmentInTransaction(runId, input));
  }

  private admitAssignmentInTransaction(runId: string, input: { name: string; role: unknown; model: AssignmentModel; coordinator: boolean; input: string; brief?: AssignmentBrief }) {
    const name = z.string().trim().min(1).max(100).parse(input.name);
    const role = roleSchema.parse(input.role);
    if (!role.enabled) throw new Conflict('Disabled roles cannot receive new assignments.');
    const instruction = z.string().min(1).max(100000).parse(input.input);
    const run = this.getRun(runId);
    if (run.status !== 'awaiting_runner') throw new Conflict('Run is not accepting assignments.');
    const current = this.runtime(runId).assignments;
    if (run.purpose !== undefined && run.purpose !== 'coding' && run.purpose !== 'research' && (!input.coordinator || current.length)) throw new Conflict('Refinement runs admit one coordinator only.');
    if (current.some(item => item.name === name)) throw new Conflict('Assignment name already admitted.');
    if (input.coordinator ? current.some(item => item.coordinator) : current.filter(item => !item.coordinator && !item.retiredAt).length >= 3) throw new Conflict('Assignment concurrency limit reached.');
    const attemptId = randomUUID();
    const assignment: Assignment = { id: randomUUID(), runId, name, role, model: structuredClone(input.model), coordinator: input.coordinator, currentAttemptId: attemptId, brief: structuredClone(input.brief ?? { criteria: [], dependsOn: [] }), projectGuidance: this.currentGuidance(runId) };
    const attempt: Attempt = { id: attemptId, assignmentId: assignment.id, runId, generation: 1, input: instruction, model: structuredClone(assignment.model), state: 'admitted', createdAt: new Date(this.clock()).toISOString() };
    this.db.prepare('INSERT INTO assignments VALUES(?,?,?,?)').run(assignment.id, runId, name, JSON.stringify(assignment));
    this.db.prepare('INSERT INTO attempts VALUES(?,?,?)').run(attempt.id, assignment.id, JSON.stringify(attempt));
    if (!current.length && run.position !== undefined) { run.position = undefined; this.saveRun(run); }
    return { assignment, attempt };
  }

  /** An interrupted launch reuses the exact admitted team and attempt identities. */
  admitInitialTeam(runId: string, team: { name: string; role: Role; model: AssignmentModel; coordinator: boolean; input: string; brief: AssignmentBrief }[]) {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.status !== 'awaiting_runner' || run.desiredState === 'cancelled' || run.desiredState === 'paused') throw new Conflict('Run stopped before initial team admission.');
      const existing = this.runtime(runId).assignments;
      if (existing.length) {
        if (existing.length !== team.length || existing.some((assignment, index) =>
          assignment.name !== team[index].name || assignment.role.id !== team[index].role.id || assignment.coordinator !== team[index].coordinator
          || this.getAttempt(assignment.currentAttemptId).input !== team[index].input)) throw new Conflict('Admitted team differs from the durable launch decision.');
        return existing.map(assignment => ({ assignment, attempt: this.getAttempt(assignment.currentAttemptId) }));
      }
      this.validateInsertionTeam(runId, team);
      return team.map(item => this.admitAssignmentInTransaction(runId, item));
    });
  }

  teamProposals(runId: string): TeamProposal[] {
    return this.db.prepare('SELECT data FROM team_proposals WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)));
  }

  insertionReplay(runId: string, value: InsertionRequest): TeamProposal | undefined {
    const previous = this.teamProposals(runId).find(item => item.origin === 'operator' && item.toolCallId === value.requestId);
    if (previous && JSON.stringify(previous.request) !== JSON.stringify(value)) throw new Conflict('Insertion request identity has different content.');
    return previous;
  }

  /** Reservations protect queued launch edits and admission as well as running teams. */
  validateInsertionTeam(runId: string, team: { name: string; coordinator: boolean; brief?: AssignmentBrief }[]) {
    const pending = this.teamProposals(runId).filter(item => item.origin === 'operator' && item.status === 'pending').flatMap(item => item.specialists);
    if (team.filter(item => !item.coordinator).length + pending.length > 3) throw new Conflict('The specialist concurrency limit includes pending insertions.');
    if (new Set([...team, ...pending].map(item => item.name)).size !== team.length + pending.length) throw new Conflict('Assignment names must be distinct.');
    const run = this.getRun(runId);
    snapshotBriefs(pending.map(item => ({ name: item.name, ...item.brief })), { criteria: run.taskSnapshot.criteria, checkIds: this.configuredCheckIds(runId), existing: team });
  }

  proposeOperatorSpecialist(runId: string, input: unknown, role: Role, model: AssignmentModel): TeamProposal {
    const value = insertionRequest.parse(input);
    return this.transaction(() => {
      const replay = this.insertionReplay(runId, value);
      if (replay) return replay;
      const run = this.getRun(runId);
      if (run.desiredState === 'cancelled' || ['cancelled', 'closed', 'completed'].includes(run.status)) throw new Conflict('This run has ended. Specialist insertion is unavailable.');
      if (!['coding', 'research'].includes(run.purpose ?? 'coding')) throw new Conflict('Refinement runs cannot insert specialists.');
      if (role.id !== value.roleId || !roleSchema.parse(role).enabled) throw new Conflict('Selected specialist role is disabled or unavailable.');
      if (run.purpose === 'research' && ['implementer', 'reviewer'].includes(role.id)) throw new Conflict('Research cannot admit coding specialists.');
      if (run.purpose === 'research' && role.id !== 'delivery-reporter' && !value.criteria.length) throw new Conflict('Research specialists need an accepted criterion obligation in their brief.');
      const proposals = this.teamProposals(runId);
      if (proposals.some(item => item.status === 'pending')) throw new Conflict('Apply the existing team proposal before inserting another specialist.');
      const assignments = this.runtime(runId).assignments;
      const existing = assignments.length ? assignments : (run.launch?.team ?? [{ name: 'Coordinator', coordinator: true }]);
      if (existing.filter(item => !item.coordinator && !('retiredAt' in item && item.retiredAt)).length >= 3) throw new Conflict('The specialist concurrency limit is three plus the coordinator.');
      let number = 1;
      const usedNames = new Set([...existing, ...proposals.flatMap(item => item.specialists)].map(item => item.name));
      while (usedNames.has(`Specialist${number}`)) number++;
      const name = `Specialist${number}`;
      const brief = snapshotBriefs([{ name, dependsOn: value.dependsOn, criteria: value.criteria.map(index => ({ index, obligation: value.input.slice(0, 2000), checkIds: [] })) }],
        { criteria: run.taskSnapshot.criteria, checkIds: this.configuredCheckIds(runId), existing }).get(name)!;
      const proposal: TeamProposal = { id: randomUUID(), runId, toolCallId: value.requestId, origin: 'operator', request: value, requirementRevision: run.taskRevision,
        createdAt: new Date(this.clock()).toISOString(), specialists: [{ name, role: structuredClone(role), model: structuredClone(model), input: value.input, brief }], status: 'pending' };
      this.db.prepare('INSERT INTO team_proposals VALUES(?,?,?,?,?)').run(proposal.id, runId, null, value.requestId, JSON.stringify(proposal));
      const notice: DeliveryNotice = { id: randomUUID(), runId, createdAt: proposal.createdAt!, disposition: 'pending', payload: {
        kind: 'operator_specialist_proposed', subjectId: proposal.id, specialist: proposal.specialists[0],
        instruction: 'The operator requested this specialist under the accepted requirements. Crewroom applies it at a safe coordination point; do not duplicate it. Reconcile its results, then retire the assignment through propose_team with a reason. Readiness waits until that responsibility is closed. Changed code still requires checks and independent review.' } };
      this.db.prepare('INSERT INTO delivery_notices VALUES(?,?,?)').run(notice.id, runId, JSON.stringify(notice));
      return proposal;
    });
  }

  insertionBlockers(runId: string): string[] {
    const assignments = this.runtime(runId).assignments;
    return this.teamProposals(runId).filter(item => item.origin === 'operator').flatMap(proposal => {
      if (proposal.status === 'pending') return ['Operator specialist insertion awaits a safe coordination point.'];
      if (proposal.status !== 'applied') return [];
      const open = assignments.filter(item => proposal.assignmentIds?.includes(item.id) && !item.retiredAt);
      return open.length ? [`Readiness waits for the coordinator to reconcile and close the inserted assignment: ${open.map(item => item.role.name).join(', ')}.`] : [];
    });
  }

  proposeTeam(attemptId: string, toolCallId: string, input: (Omit<ProposedSpecialist, 'brief'> & { brief?: ProposedSpecialist['brief'] })[], retire: { name: string; reason: string }[] = []): TeamProposal {
    z.string().min(1).max(200).parse(toolCallId);
    const specialists = z.array(z.object({ name: z.string().regex(/^[A-Za-z][A-Za-z0-9_-]{0,60}$/), role: roleSchema,
      model: z.custom<ProposedSpecialist['model']>(), input: z.string().trim().min(1).max(40000), brief: z.custom<ProposedSpecialist['brief']>(value => !!value && typeof value === 'object').default({ criteria: [], dependsOn: [] }) }).strict()).max(3).parse(input);
    if (specialists.some(item => !item.role.enabled)) throw new Conflict('Disabled roles cannot receive new assignments.');
    const retirementRequests = z.array(z.object({ name: z.string(), reason: z.string().trim().min(1).max(2000) }).strict()).max(3).parse(retire);
    if (!specialists.length && !retirementRequests.length) throw new Conflict('Propose at least one team change.');
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      if (this.getRun(attempt.runId).purpose && !['coding', 'research'].includes(this.getRun(attempt.runId).purpose!)) throw new Conflict('Refinement cannot propose specialists.');
      const runtime = this.runtime(attempt.runId);
      if (!runtime.assignments.some(item => item.coordinator && item.currentAttemptId === attemptId) || attempt.ownership !== 'held') throw new Conflict('Only the active coordinator can propose specialists.');
      const retirements = retirementRequests.map(request => {
        const target = runtime.assignments.find(item => item.name === request.name);
        if (!target || target.coordinator) throw new Conflict('Only an admitted specialist can retire.');
        return { assignmentId: target.id, reason: request.reason };
      });
      if (new Set(retirements.map(item => item.assignmentId)).size !== retirements.length) throw new Conflict('Retirement targets must be distinct.');
      const proposals = this.teamProposals(attempt.runId);
      const previous = proposals.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
      if (previous) {
        if (JSON.stringify(previous.specialists) !== JSON.stringify(specialists) || JSON.stringify(previous.retirements ?? []) !== JSON.stringify(retirements)) throw new Conflict('Proposal identity has different content.');
        return previous;
      }
      if (this.getRun(attempt.runId).status !== 'running') throw new Conflict('The run is not accepting proposals.');
      if (proposals.some(item => item.status === 'pending')) throw new Conflict('The concurrency limit includes pending proposals; apply the existing change first.');
      const writer = this.repositoryWork(attempt.runId)?.writer;
      if (retirements.some(target => {
        const assignment = runtime.assignments.find(item => item.id === target.assignmentId)!;
        return assignment.retiredAt || writer?.attemptId === assignment.currentAttemptId;
      })) throw new Conflict('A retired specialist or current writer cannot retire.');
      if (runtime.assignments.filter(item => !item.retiredAt).length - retirements.length + specialists.length > 4) throw new Conflict('The specialist concurrency limit is three plus the coordinator.');
      const names = [...runtime.assignments, ...specialists].map(item => item.name);
      if (new Set(names).size !== names.length) throw new Conflict('Assignment names must be distinct.');
      const proposal: TeamProposal = { id: randomUUID(), runId: attempt.runId, attemptId, toolCallId, specialists: structuredClone(specialists), retirements, status: 'pending' };
      this.db.prepare('INSERT INTO team_proposals VALUES(?,?,?,?,?)').run(proposal.id, proposal.runId, attemptId, toolCallId, JSON.stringify(proposal));
      return proposal;
    });
  }
  /** Reject a now-unavailable proposed role before stopping agents; replay keeps one decision and one notice. */
  rejectUnavailableTeamProposal(id: string, currentRoles: Role[]): TeamProposal | undefined {
    const row = this.db.prepare('SELECT data FROM team_proposals WHERE id=?').get(id);
    if (!row) throw new NotFound('Team proposal not found.');
    const proposal: TeamProposal = JSON.parse(String(row.data));
    if (proposal.status === 'rejected') return proposal;
    if (proposal.status !== 'pending') return undefined;
    const changed = proposal.origin === 'operator' && proposal.requirementRevision !== this.getRun(proposal.runId).taskRevision;
    if (!changed && (!currentRoles.length || !proposal.specialists.some(item => !currentRoles.find(role => role.id === item.role.id)?.enabled))) return undefined;
    return this.transaction(() => {
      const currentRow = this.db.prepare('SELECT data FROM team_proposals WHERE id=?').get(id);
      if (!currentRow) throw new NotFound('Team proposal not found.');
      const current: TeamProposal = JSON.parse(String(currentRow.data));
      if (current.status === 'pending' && this.rejectUnavailableTeamProposalInTransaction(current, currentRoles)) return current;
      return current.status === 'rejected' ? current : undefined;
    });
  }

  private rejectUnavailableTeamProposalInTransaction(proposal: TeamProposal, currentRoles: Role[]): boolean {
    const unavailable = currentRoles.length && proposal.specialists.find(item => !currentRoles.find(role => role.id === item.role.id)?.enabled);
    const changed = proposal.origin === 'operator' && proposal.requirementRevision !== this.getRun(proposal.runId).taskRevision;
    if (!unavailable && !changed) return false;
    proposal.status = 'rejected';
    proposal.reason = changed ? 'Accepted requirements changed before insertion. Submit a new specialist proposal for the current requirements.'
      : `Selected role ${unavailable && unavailable.role.name} is disabled or unavailable. No specialists from this proposal were admitted; choose available roles and propose a new team.`;
    this.db.prepare('UPDATE team_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), proposal.id);
    // A still-undelivered request must not instruct the coordinator to execute a now-rejected brief.
    for (const pending of this.deliveryNotices(proposal.runId).filter(item => item.payload.subjectId === proposal.id && item.payload.kind === 'operator_specialist_proposed' && item.disposition === 'pending')) {
      pending.payload = { kind: pending.payload.kind, subjectId: proposal.id, status: 'rejected', reason: proposal.reason,
        instruction: 'This operator proposal was rejected before admission. Retain its history; wait for a new operator request rather than recreating it.' };
      this.db.prepare('UPDATE delivery_notices SET data=? WHERE id=?').run(JSON.stringify(pending), pending.id);
    }
    const payload = { kind: 'team_proposal_rejected', subjectId: proposal.id, reason: proposal.reason, origin: proposal.origin ?? 'coordinator' };
    const notice: DeliveryNotice = { id: randomUUID(), runId: proposal.runId, payload, createdAt: new Date().toISOString(), disposition: 'pending' };
    this.db.prepare('INSERT INTO delivery_notices VALUES(?,?,?)').run(notice.id, proposal.runId, JSON.stringify(notice));
    return true;
  }


  /**
   * `currentRoles` is the user catalog at application time. A specialist is admitted with the role's current
   * version when its model and effort still match the validated proposal; otherwise the proposal snapshot stands.
   */
  applyTeamProposal(id: string, currentRoles: Role[] = []): TeamProposal {
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM team_proposals WHERE id=?').get(id);
      if (!row) throw new NotFound('Team proposal not found.');
      const proposal: TeamProposal = JSON.parse(String(row.data));
      if (proposal.status !== 'pending') return proposal;
      const run = this.getRun(proposal.runId);
      const runtime = this.runtime(run.id);
      if (run.status !== 'paused' || run.desiredState !== 'running' || runtime.attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped')) throw new Conflict('Pause and confirm old ownership before inserting specialists.');
      if (runtime.messages.some(message => message.disposition !== 'received') || this.steering(run.id).some(message => message.delivery === 'dispatched')) throw new Conflict('Reconcile pending delivery before inserting specialists.');
      const retirements = proposal.retirements ?? [];
      if (runtime.assignments.filter(item => !item.retiredAt).length - retirements.length + proposal.specialists.length > 4) throw new Conflict('The specialist concurrency limit is three plus the coordinator.');
      if (this.rejectUnavailableTeamProposalInTransaction(proposal, currentRoles)) return proposal;
      for (const target of retirements) {
        const assignment = runtime.assignments.find(item => item.id === target.assignmentId);
        if (!assignment || assignment.coordinator || assignment.retiredAt || this.repositoryWork(run.id)?.writer?.attemptId === assignment.currentAttemptId) throw new Conflict('Specialist retirement is no longer valid.');
        assignment.retiredAt = new Date().toISOString(); assignment.retirementReason = target.reason;
        this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignment.id);
      }
      proposal.assignmentIds = [];
      for (const specialist of proposal.specialists) {
        const attemptId = randomUUID();
        const current = currentRoles.find(role => role.id === specialist.role.id);
        const role = current && current.enabled && current.model === specialist.role.model && current.effort === specialist.role.effort ? roleSchema.parse(current) : specialist.role;
        const assignment: Assignment = { id: randomUUID(), runId: run.id, name: specialist.name, role, model: specialist.model, coordinator: false, currentAttemptId: attemptId, brief: specialist.brief, projectGuidance: this.currentGuidance(run.id) };
        const attempt: Attempt = { id: attemptId, assignmentId: assignment.id, runId: run.id, generation: 1, input: specialist.input, model: structuredClone(assignment.model), state: 'admitted', createdAt: new Date(this.clock()).toISOString() };
        this.db.prepare('INSERT INTO assignments VALUES(?,?,?,?)').run(assignment.id, run.id, assignment.name, JSON.stringify(assignment));
        this.db.prepare('INSERT INTO attempts VALUES(?,?,?)').run(attempt.id, assignment.id, JSON.stringify(attempt));
        proposal.assignmentIds.push(assignment.id);
      }
      proposal.status = 'applied';
      this.db.prepare('UPDATE team_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), id);
      return proposal;
    });
  }

  beginCodingTransition(runId: string, completionId: string) {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.status !== 'running' || run.desiredState !== 'running' || !this.codingRecord(runId)?.completions?.some(item => item.id === completionId && item.state === 'requested')) throw new Conflict('Coding completion is not ready for transition.');
      run.codingTransition = { completionId, controlRevision: run.controlRevision! }; return this.saveRun(run);
    });
  }
  finishCodingTransition(runId: string) {
    return this.transaction(() => { const run = this.getRun(runId); run.codingTransition = undefined; return this.saveRun(run); });
  }

  beginTeamTransition(proposalId: string) {
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM team_proposals WHERE id=?').get(proposalId);
      if (!row) throw new NotFound('Team proposal not found.');
      const proposal: TeamProposal = JSON.parse(String(row.data));
      const run = this.getRun(proposal.runId);
      if (run.status !== 'running' || run.desiredState !== 'running') throw new Conflict('The run is not executing.');
      run.teamTransition = { proposalId, controlRevision: run.controlRevision! };
      return this.saveRun(run);
    });
  }

  finishTeamTransition(runId: string) {
    return this.transaction(() => { const run = this.getRun(runId); run.teamTransition = undefined; return this.saveRun(run); });
  }

  prepareMessage(attemptId: string, toolCallId: string, input: unknown): PeerMessage {
    const value = z.object({ to: z.string(), body: z.string().min(1).max(40000), replyTo: z.string().optional() }).strict().parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM attempts WHERE id=?').get(attemptId);
      if (!row) throw new NotFound('Attempt not found.');
      const attempt = JSON.parse(String(row.data)) as Attempt;
      const runtime = this.runtime(attempt.runId);
      const sender = runtime.assignments.find(item => item.currentAttemptId === attemptId);
      if (!sender || sender.retiredAt || ['failed', 'exited', 'attention_required'].includes(attempt.state)) throw new Conflict('Sender attempt no longer owns this assignment.');
      const recipient = runtime.assignments.find(item => item.id === value.to || item.name === value.to);
      if (!recipient || recipient.retiredAt) throw new Conflict('Unknown or retired recipient in this run.');
      if (value.replyTo && !runtime.messages.some(item => item.id === value.replyTo && item.to === sender.id && ['accepted', 'dispatched', 'received'].includes(item.disposition))) throw new Conflict('Reply does not refer to an accepted message for this sender.');
      const existing = runtime.messages.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
      if (existing) {
        if (existing.body !== value.body || existing.to !== recipient.id || existing.replyTo !== value.replyTo) throw new Conflict('Message key has different content.');
        return existing;
      }
      if (runtime.messages.filter(item => item.to === recipient.id && item.disposition !== 'received').length >= pendingMessageLimit)
        throw new Conflict(`Recipient backlog reached ${pendingMessageLimit} pending messages. Retained messages must be received before sending more.`);
      const message: PeerMessage = { id: randomUUID(), runId: attempt.runId, attemptId, toolCallId,
        from: sender.id, to: recipient.id, body: value.body, replyTo: value.replyTo,
        createdAt: new Date().toISOString(), disposition: 'pending' };
      this.db.prepare('INSERT INTO peer_messages VALUES(?,?,?,?,?)').run(message.id, message.runId, attemptId, toolCallId, JSON.stringify(message));
      return message;
    });
  }

  bindMailbox(assignmentId: string, mailboxName: string) {
    z.string().regex(/^[A-Za-z]+$/).parse(mailboxName);
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM assignments WHERE id=?').get(assignmentId);
      if (!row) throw new NotFound('Assignment not found.');
      const assignment = JSON.parse(String(row.data)) as Assignment;
      if (assignment.mailboxName && assignment.mailboxName !== mailboxName) throw new Conflict('Mailbox identity already bound.');
      assignment.mailboxName = mailboxName;
      this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignmentId);
    });
  }

  getPeerMessage(id: string): PeerMessage {
    const row = this.db.prepare('SELECT data FROM peer_messages WHERE id=?').get(id);
    if (!row) throw new NotFound('Message not found.');
    return JSON.parse(String(row.data));
  }

  recordMessageStored(id: string, transportId: number) {
    z.number().int().positive().parse(transportId);
    return this.transaction(() => {
      const message = this.getPeerMessage(id);
      if (message.transportId && message.transportId !== transportId) throw new Conflict('Transport identity changed. Reconciliation required.');
      if (!message.transportId) {
        message.transportId = transportId; message.disposition = 'stored';
        this.db.prepare('UPDATE peer_messages SET data=? WHERE id=?').run(JSON.stringify(message), id);
      }
      return message;
    });
  }

  acceptMessage(id: string) {
    return this.transaction(() => {
      const message = this.getPeerMessage(id);
      if (!message.transportId || message.disposition === 'uncertain') throw new Conflict('Message storage is not confirmed.');
      if (message.disposition === 'stored') {
        message.disposition = 'accepted';
        this.db.prepare('UPDATE peer_messages SET data=? WHERE id=?').run(JSON.stringify(message), id);
      }
      return message;
    });
  }

  markMessageUncertain(id: string) {
    const message = this.getPeerMessage(id); message.disposition = 'uncertain';
    this.db.prepare('UPDATE peer_messages SET data=? WHERE id=?').run(JSON.stringify(message), id);
  }

  restoreUndispatchedInput(attemptId: string, inputId: string) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      if (attempt.ownership !== 'stopped') throw new Conflict('Confirm old ownership stopped before restoring undispatched input.');
      for (const message of this.runtime(attempt.runId).messages) if (message.id === inputId && message.disposition === 'dispatched' && message.recipientAttemptId === attemptId) {
        message.disposition = 'accepted'; message.recipientAttemptId = undefined;
        this.db.prepare('UPDATE peer_messages SET data=? WHERE id=?').run(JSON.stringify(message), message.id);
      }
      for (const message of this.steering(attempt.runId)) if (message.inputId === inputId && message.delivery === 'dispatched' && message.attemptId === attemptId) {
        const reason = steeringUndeliveredReason(this.getRun(attempt.runId));
        message.inputId = undefined; message.attemptId = undefined;
        this.transitionSteering(message, reason ? 'not_delivered' : 'pending',
          reason ?? 'Stopped host has no saved input; waiting for delivery.');
      }
      const notice = this.deliveryNotices(attempt.runId).find(item => item.id === inputId && item.disposition === 'dispatched' && item.attemptId === attemptId);
      if (notice) { notice.disposition = 'pending'; notice.attemptId = undefined; this.db.prepare('UPDATE delivery_notices SET data=? WHERE id=?').run(JSON.stringify(notice), notice.id); }
    });
  }

  observerCursor(assignmentId: string) {
    return Number(this.db.prepare('SELECT cursor FROM observer_cursors WHERE assignment_id=?').get(assignmentId)?.cursor ?? 0);
  }

  getAttempt(id: string): Attempt {
    const row = this.db.prepare('SELECT data FROM attempts WHERE id=?').get(id);
    if (!row) throw new NotFound('Attempt not found.');
    return JSON.parse(String(row.data));
  }

  claimAttempt(id: string) {
    return this.transaction(() => {
      const attempt = this.getAttempt(id);
      if (attempt.state !== 'admitted') throw new Conflict('Attempt already claimed; reconcile ownership before replacement.');
      const conflict = executionConflict({ runId: attempt.runId, coding: !!this.repositoryWork(attempt.runId) }, this.executingRuns());
      if (conflict) throw new Conflict(conflict);
      attempt.state = 'starting';
      this.db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt), id);
      this.accountTimeInTransaction(attempt.runId);
      return attempt;
    });
  }

  /** Runs with a claimed attempt whose process ownership is not confirmed stopped. */
  executingRuns(): ExecutingRun[] {
    return this.db.prepare(`SELECT DISTINCT s.run_id AS run_id, EXISTS(SELECT 1 FROM repository_work w WHERE w.run_id=s.run_id) AS coding
      FROM attempts a JOIN assignments s ON a.assignment_id=s.id WHERE a.state<>'admitted' AND a.ownership IS NOT 'stopped'`).all()
      .map(row => ({ runId: String(row.run_id), coding: !!row.coding }));
  }

  /** Includes legacy manual coding runs without repository work; uncertainty also blocks a successor. */
  unstoppedCodingRun(exceptRunId = ''): Run | undefined {
    const row = this.db.prepare(`SELECT s.run_id FROM attempts a JOIN assignments s ON a.assignment_id=s.id JOIN runs r ON r.id=s.run_id
      WHERE s.run_id<>? AND COALESCE(json_extract(r.data,'$.purpose'),'coding')='coding'
      AND a.state<>'admitted' AND a.ownership IS NOT 'stopped' LIMIT 1`).get(exceptRunId);
    return row ? this.getRun(String(row.run_id)) : undefined;
  }

  replaceAttempt(assignmentId: string, input: { automatic: boolean; fallback?: { model: AssignmentModel; eventSequence: number } }): Attempt {
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM assignments WHERE id=?').get(assignmentId);
      if (!row) throw new NotFound('Assignment not found.');
      const assignment: Assignment = JSON.parse(String(row.data));
      if (assignment.retiredAt) throw new Conflict('Retired assignments cannot resume.');
      const previous = this.getAttempt(assignment.currentAttemptId);
      if (previous.ownership !== 'stopped') throw new Conflict('Resolve old process ownership before replacement.');
      const run = this.getRun(assignment.runId);
      if (run.status !== 'paused') throw new Conflict('Pause and reconcile the run before replacement.');
      if ((run.activeMs ?? 0) >= (run.activeLimitMs ?? 3600000)) throw new Conflict('The active execution budget is exhausted.');
      if (this.runtime(run.id).messages.some(message => ['pending', 'dispatched', 'uncertain'].includes(message.disposition))) throw new Conflict('Reconcile uncertain message effects before replacement.');
      if (this.steering(run.id).some(message => message.delivery === 'dispatched')) throw new Conflict('Reconcile uncertain steering delivery before replacement.');
      if (this.deliveryNotices(run.id).some(item => item.disposition === 'dispatched')) throw new Conflict('Reconcile uncertain controller delivery before replacement.');
      if (input.automatic && (assignment.automaticCycles ?? 0) >= automaticCycleLimit(assignment)) throw new Conflict(cyclesExhausted(assignment, 'recovery'));
      const previousModel = previous.model ?? assignment.model;
      const failure = input.fallback && this.agentEventsOfType(run.id, ['prompt_result']).find(item => item.attemptId === previous.id && item.sequence === input.fallback!.eventSequence);
      if (input.fallback && (!input.automatic || !failure || failure.event.status !== 'error' || failure.event.error?.retryable !== true
        || failure.event.error?.provider !== previousModel.resolved.provider || failure.event.error?.model !== previousModel.resolved.id
        || input.fallback.model.fallbackFrom !== previousModel.resolved.selector || input.fallback.model.resolved.selector === previousModel.resolved.selector))
        throw new Conflict('A model fallback requires the prior attempt\'s recorded retryable provider failure.');
      const attempt: Attempt = { id: randomUUID(), assignmentId, runId: assignment.runId, generation: previous.generation + 1,
        input: previous.input, model: structuredClone(input.fallback?.model ?? previousModel),
        ...(input.fallback ? { fallback: { fromAttemptId: previous.id, fromModel: previousModel.resolved.selector, eventSequence: input.fallback.eventSequence, reason: String(failure!.event.error.message ?? 'Retryable provider failure') } } : {}),
        state: 'admitted', createdAt: new Date(this.clock()).toISOString(), previousAttemptId: previous.id, activeInputMessageId: previous.activeInputMessageId };
      assignment.currentAttemptId = attempt.id;
      assignment.automaticCycles = (assignment.automaticCycles ?? 0) + (input.automatic ? 1 : 0);
      this.db.prepare('INSERT INTO attempts VALUES(?,?,?)').run(attempt.id, assignmentId, JSON.stringify(attempt));
      this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignment.id);
      return attempt;
    });
  }

  /** Records an explicit operator decision to permit one more automatic repair/replacement cycle. */
  grantRepairCycle(runId: string, assignmentId: string, input: unknown) {
    const { reason, requestId } = z.object({ reason: z.string().trim().min(1).max(2000), requestId: z.string().uuid() }).strict().parse(input);
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM assignments WHERE id=? AND run_id=?').get(assignmentId, runId);
      if (!row) throw new NotFound('Assignment not found.');
      const assignment: Assignment = JSON.parse(String(row.data));
      if (assignment.retiredAt) throw new Conflict('Retired assignments cannot receive repair cycles.');
      if (this.getRun(runId).desiredState === 'cancelled') throw new Conflict('This run is cancelled.');
      if (assignment.repairAllowances?.some(item => item.id === requestId)) return assignment;
      if ((assignment.automaticCycles ?? 0) < automaticCycleLimit(assignment)) throw new Conflict('Automatic cycles remain for this assignment; no extra allowance is needed.');
      assignment.repairAllowances = [...(assignment.repairAllowances ?? []), { id: requestId, grantedAt: new Date(this.clock()).toISOString(), reason }];
      this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignment.id);
      return assignment;
    });
  }

  recordAttemptIdentity(id: string, identity: Required<Pick<Attempt, 'hostPid' | 'ompPid' | 'sessionId' | 'hostUrl' | 'pane'>> & Pick<Attempt, 'recoveryMode' | 'sessionDirectory' | 'runnerCwd'>) {
    return this.transaction(() => {
      const attempt = this.getAttempt(id);
      if (attempt.state !== 'starting') throw new Conflict('Attempt is not awaiting identity verification.');
      Object.assign(attempt, identity, { state: 'idle', ownership: 'held', startedAt: new Date(this.clock()).toISOString() });
      this.db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt), id);
      return attempt;
    });
  }

  recordDispatch(messageId: string, received = false) {
    return this.transaction(() => {
      const message = this.getPeerMessage(messageId);
      if (!['accepted', 'dispatched', 'received'].includes(message.disposition)) throw new Conflict('Message has not been accepted.');
      if (message.disposition === 'received') return;
      message.disposition = received ? 'received' : 'dispatched';
      message.recipientAttemptId ??= this.runtime(message.runId).assignments.find(item => item.id === message.to)!.currentAttemptId;
      this.db.prepare('UPDATE peer_messages SET data=? WHERE id=?').run(JSON.stringify(message), messageId);
      const assignment = this.runtime(message.runId).assignments.find(item => item.id === message.to)!;
      const attempt = this.getAttempt(assignment.currentAttemptId);
      attempt.activeInputMessageId = messageId;
      this.db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt), attempt.id);
    });
  }

  private changeRun(runId: string, action: Parameters<typeof transitionRun>[1]) {
    try { return this.saveRun(transitionRun(this.getRun(runId), action, this.clock())); }
    catch (error) { if (error instanceof LifecycleConflict) throw new Conflict(error.message); throw error; }
  }

  private saveRun(run: Run) {
    const before = this.getRun(run.id);
    const eligible = (run.purpose ?? 'coding') === 'coding' && !run.schedule && run.status === 'awaiting_runner'
      && !this.db.prepare('SELECT 1 FROM assignments WHERE run_id=? LIMIT 1').get(run.id);
    if (!eligible) run.position = undefined;
    else if (run.position === undefined) run.position = this.codingQueue().plans.length;
    this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(run), run.id);
    if (run.status === 'cancelled' && run.continuedFrom && !this.repositoryWork(run.id)) {
      const parent = this.getRun(run.continuedFrom.runId);
      if (parent.continuedBy === run.id && !this.repositoryWork(parent.id)?.transferredTo) {
        parent.continuedBy = undefined; this.saveRun(parent);
      }
    }
    this.settleUndeliveredSteering(run, new Date(this.clock()).toISOString());
    if (run.desiredState === 'cancelled' || ['cancelled', 'closed', 'completed'].includes(run.status)) {
      for (const proposal of this.teamProposals(run.id).filter(item => item.origin === 'operator' && item.status === 'pending')) {
        proposal.status = 'rejected'; proposal.reason = 'The run ended before this specialist was admitted.';
        this.db.prepare('UPDATE team_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), proposal.id);
      }
    }
    if (before.position !== undefined && run.position === undefined) {
      for (const plan of this.codingQueue().plans.filter(item => item.position! > before.position!)) {
        plan.position = plan.position! - 1;
        this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(plan), plan.id);
      }
      this.bumpQueueRevision();
    } else if (before.position === undefined && run.position !== undefined) this.bumpQueueRevision();
    return run;
  }

  private bumpQueueRevision() { this.db.prepare('UPDATE queue_state SET revision=revision+1 WHERE id=1').run(); }

  /** Only awaiting, unadmitted coding plans; local positions never follow Jira rank. */
  codingQueue(): CodingQueue {
    const holder = this.codingSlot();
    const unstopped = !holder && this.unstoppedCodingRun();
    return { revision: Number(this.db.prepare('SELECT revision FROM queue_state WHERE id=1').get()!.revision),
      ...(holder ? { slot: { runId: holder.id, key: holder.taskSnapshot.key, reason: this.slotReason(holder), decisionId: holder.schedule!.id, decidedAt: holder.schedule!.at } } : {}),
      plans: this.db.prepare("SELECT data FROM runs WHERE json_extract(data,'$.position') IS NOT NULL ORDER BY CAST(json_extract(data,'$.position') AS INTEGER)").all()
        .map(row => JSON.parse(String(row.data)) as Run & { holdReason?: string }).map(plan => {
          const { holdReason: _stale, ...saved } = plan;
          const reason = this.eligibilityHold(saved) ?? (holder ? `Waiting for ${holder.taskSnapshot.key} to reach ready for review (${this.slotReason(holder)})`
            : unstopped ? `Waiting for ${unstopped.taskSnapshot.key}'s agents to stop; process ownership needs reconciliation` : undefined);
          return reason ? { ...saved, holdReason: reason } : saved;
        }) };
  }

  codingSlot(): Run | undefined {
    const row = this.db.prepare('SELECT run_id FROM coding_slot WHERE id=1').get();
    return row?.run_id ? this.getRun(String(row.run_id)) : undefined;
  }

  private readinessHold(run: Run, holderId?: string, publication = this.publication(run.id)): Run['readinessHold'] {
    if (run.slotRelease?.reason !== 'review' || !['paused', 'attention_required'].includes(run.status)
      || run.desiredState !== 'paused' || publication?.merged || (!run.sourceConflict && !publication?.reviewInvalidated)) return undefined;
    return { state: holderId === run.id ? 'held' : 'pending', candidateId: publication?.candidateId ?? run.slotRelease.candidateId!, since: run.slotRelease.at };
  }
  private slotReason(run: Run): string {
    if (run.status === 'attention_required') return `${run.taskSnapshot.key} needs attention`;
    const readinessHold = this.readinessHold(run, this.codingSlot()?.id);
    if (readinessHold?.state === 'held') return `${run.taskSnapshot.key} readiness is unconfirmed; Resume is explicit`;
    if (run.status === 'paused' || run.status === 'pausing') {
      const state = run.status === 'pausing' ? 'pausing' : 'paused';
      switch (run.pauseReason) {
        case 'operator': return `${state} by operator`;
        case 'budget': return `${state} at the active-time limit`;
        case 'recovery': return `${state} for recovery`;
        case 'requirements': return `${state} for requirements adoption`;
        case 'source': return `${state} for a source conflict`;
        case 'review': return `${state} awaiting human review`;
        case 'validation': return `${state}; readiness is not confirmed`;
        case 'merged': return `PR merged; ${state}`;
        default: return `${state}, awaiting readiness`;
      }
    }
    return run.status === 'running' ? 'running' : 'awaiting readiness';
  }

  /** Dependencies remain live for unstarted plans; accepted brief edits still require adoption. */
  eligibilityHold(run: Run): string | undefined {
    const task = this.getTask(run.taskId);
    if (run.sourceConflict || task.needsReacceptance || run.taskSnapshot.sourceRevision !== task.sourceRevision
      || run.taskSnapshot.brief !== task.brief || JSON.stringify(run.taskSnapshot.criteria) !== JSON.stringify(task.criteria)) return 'Requirements changed';
    if (!run.taskSnapshot.repositoryId) return 'Needs launch details';
    if (!this.db.prepare("SELECT 1 FROM repositories WHERE id=? AND json_extract(data,'$.retiredAt') IS NULL").get(run.taskSnapshot.repositoryId)) return 'Selected repository unavailable';
    if (!run.taskSnapshot.brief.trim() || !run.taskSnapshot.criteria.length) return 'Needs a current brief';
    if (task.dependsOn?.length) {
      const hold = dependencyHold(run, task, this.publications(), this.allTasks(), this.allRuns());
      if (hold) return hold;
      if (run.dependencyBaseHold?.merges === JSON.stringify(this.dependencyMerges(run)) && run.dependencyBaseHold.reason)
        return run.dependencyBaseHold.reason;
    }
    return run.launchHold?.reason ?? (!run.launch ? 'Needs launch details' : undefined);
  }
  dependencyMerges(run: Run): { key: string; commit: string }[] {
    const task = this.getTask(run.taskId);
    if (!task.dependsOn?.length) return [];
    const runs = this.allRuns(); const publications = this.publications();
    return (task.dependsOn ?? []).flatMap(id => {
      const dependency = this.getTask(id);
      const delivered = runs.filter(item => item.taskId === id && (item.purpose ?? 'coding') === 'coding'
        && item.taskSnapshot.repositoryId === run.taskSnapshot.repositoryId)
        .map(item => publications.find(record => record.runId === item.id && !!record.merged)).find(Boolean);
      return delivered?.merged ? [{ key: dependency.key, commit: delivered.merged.commit }] : [];
    });
  }

  /** Refuse a stale asynchronous check after a dependency edit or publication change. */
  saveDependencyBaseHold(runId: string, merges: string, reason?: string) {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.position === undefined || JSON.stringify(this.dependencyMerges(run)) !== merges) return false;
      run.dependencyBaseHold = { merges, reason };
      this.saveRun(run);
      return true;
    });
  }

  /** An outdated preflight cannot overwrite an edited launch spec. */
  setLaunchHold(runId: string, launch: LaunchSpec, reason?: string): boolean {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.position === undefined || JSON.stringify(run.launch) !== JSON.stringify(launch)) return false;
      if (run.launchHold && run.launchHold.source !== 'preflight') return false;
      if (run.launchHold?.reason === reason) return true;
      run.launchHold = reason ? { reason, at: new Date(this.clock()).toISOString(), source: 'preflight' } : undefined;
      this.saveRun(run);
      return true;
    });
  }

  /** Queue order and slot claim share one SQLite transaction, even across two store connections. */
  claimNextCodingSlot(): Run | undefined {
    return this.transaction(() => {
      if (this.codingSlot() || this.unstoppedCodingRun()) return undefined;
      const plan = this.codingQueue().plans.find(item => !this.eligibilityHold(item));
      return plan && this.claimCodingSlotInTransaction(plan.id, false);
    });
  }

  /** A read-only early refusal; the transactional claim checks again after preflight. */
  assertCodingSlotAvailable(runId: string): void {
    this.getRun(runId);
    const holder = this.codingSlot();
    if (holder && holder.id !== runId) throw new Conflict(`${holder.taskSnapshot.key} holds the coding slot (${this.slotReason(holder)}).`);
    const unstopped = !holder && this.unstoppedCodingRun(runId);
    if (unstopped) throw new Conflict(`${unstopped.taskSnapshot.key} still has unconfirmed coding process ownership. Wait for its agents to stop before starting another coding ticket.`);
  }

  claimCodingSlot(runId: string, manual = false): Run {
    return this.transaction(() => this.claimCodingSlotInTransaction(runId, manual));
  }

  private claimCodingSlotInTransaction(runId: string, manual: boolean): Run {
    const run = this.getRun(runId);
    if ((run.purpose ?? 'coding') !== 'coding' || run.status !== 'awaiting_runner' || run.desiredState === 'cancelled') throw new Conflict('Coding run is not available to start.');
    this.assertCodingSlotAvailable(runId);
    if (run.slotRelease) throw new Conflict('Review-ready work needs an explicit Resume; a new start cannot reclaim its released slot.');
    const hold = run.position !== undefined && this.eligibilityHold(run);
    if (hold && (!manual || (hold !== 'Needs launch details' && hold !== run.launchHold?.reason))) throw new Conflict(hold);
    run.launchHold = undefined;
    if (!run.schedule) { run.schedule = { id: randomUUID(), at: new Date(this.clock()).toISOString(), position: run.position }; this.saveRun(run); }
    this.db.prepare('UPDATE coding_slot SET run_id=? WHERE id=1').run(runId);
    return run;
  }

  releaseCodingSlot(runId: string, reason: 'review' | 'cancelled' | 'merged', candidateId?: string): Run {
    return this.transaction(() => this.releaseCodingSlotInTransaction(runId, reason, candidateId));
  }

  private releaseCodingSlotInTransaction(runId: string, reason: 'review' | 'cancelled' | 'merged', candidateId?: string): Run {
    const run = this.getRun(runId);
    if (this.codingSlot()?.id !== runId) return run;
    if (reason === 'review' && (!candidateId || !this.publication(runId)?.ready || this.publication(runId)?.candidateId !== candidateId)) throw new Conflict('Current publication readiness is required for review release.');
    if (reason === 'review' || reason === 'merged') {
      const alreadyAtReviewGate = reason === 'review' && run.status === 'paused' && run.desiredState === 'paused'
        && (run.pauseReason === 'review' || run.pauseReason === 'validation');
      if (alreadyAtReviewGate) this.changeRun(runId, { type: 'reviewRestored' });
      else {
        this.changeRun(runId, { type: 'desire', desiredState: 'paused' });
        this.changeRun(runId, { type: 'beginPause', reason });
      }
    }
    const updated = this.getRun(runId);
    updated.slotRelease = { reason, at: new Date(this.clock()).toISOString(), ...(candidateId ? { candidateId } : {}) };
    this.saveRun(updated);
    this.db.prepare('UPDATE coding_slot SET run_id=NULL WHERE id=1').run();
    return updated;
  }

  /** A rejected pre-admission decision keeps its original local priority and becomes retryable. */
  deferUnadmittedLaunch(runId: string, reason?: string, dependency = false): Run {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (this.codingSlot()?.id !== runId || run.status !== 'awaiting_runner' || this.repositoryWork(runId)
        || this.runtime(runId).assignments.length || run.desiredState === 'cancelled' || run.desiredState === 'paused')
        throw new Conflict('Cannot defer a coding run after preparation or admission.');
      this.db.prepare('UPDATE coding_slot SET run_id=NULL WHERE id=1').run();
      const queued = this.codingQueue().plans;
      const position = Math.min(run.schedule?.position ?? queued.length, queued.length);
      for (const plan of queued.filter(item => item.position! >= position)) {
        plan.position = plan.position! + 1;
        this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(plan), plan.id);
      }
      run.schedule = undefined;
      run.position = position;
      if (dependency) run.dependencyBaseHold = { merges: JSON.stringify(this.dependencyMerges(run)), reason };
      else run.launchHold = reason ? { reason, at: new Date(this.clock()).toISOString(), source: 'preparation' } : undefined;
      this.saveRun(run);
      return run;
    });
  }

  /** A target that changed between screening and prepare returns the unadmitted plan to the queue. */
  deferDependencyLaunch(runId: string, reason: string) { return this.deferUnadmittedLaunch(runId, reason, true); }

  /** A durable fence is execution evidence, not admission or a successful wrapper return. */
  canReplayInitialLaunch(runId: string, decisionId: string, controlRevision: number): boolean {
    const run = this.getRun(runId);
    return this.codingSlot()?.id === runId && run.schedule?.id === decisionId && !run.launchCompletedAt && !!run.launch
      && run.status === 'awaiting_runner' && run.desiredState !== 'paused' && run.desiredState !== 'cancelled'
      && !run.sourceConflict && (run.controlRevision ?? 0) === controlRevision
      && this.runtime(runId).assignments.every(assignment => this.getAttempt(assignment.currentAttemptId).state === 'admitted');
  }

  /** Upgrade pre-fence persisted runs only from positive prior-execution evidence. */
  normalizeLaunchExecution(runId: string): Run {
    const run = this.getRun(runId);
    if (!run.schedule || run.launchCompletedAt || (run.purpose ?? 'coding') !== 'coding') return run;
    const agentStarted = !!this.db.prepare(`SELECT 1 FROM agent_events e JOIN attempts a ON a.id=e.attempt_id
      JOIN assignments s ON s.id=a.assignment_id WHERE s.run_id=? AND json_extract(e.data,'$.type')='agent_start' LIMIT 1`).get(runId);
    if (run.status !== 'running' && run.activeSince === undefined && !(run.activeMs && run.activeMs > 0) && !agentStarted) return run;
    return this.transaction(() => {
      const current = this.getRun(runId);
      if (!current.launchCompletedAt) {
        current.launchCompletedAt = new Date(this.clock()).toISOString();
        this.saveRun(current);
      }
      return current;
    });
  }

  /** Reuses assignment identities; a stopped, interrupted initial host gets a new attempt. */
  restartScheduledLaunch(runId: string) {
    return this.transaction(() => {
      const run = this.getRun(runId); const runtime = this.runtime(runId);
      if (!run.schedule || run.launchCompletedAt || !run.launch || run.status !== 'paused' || run.pauseReason !== 'recovery'
        || run.desiredState !== 'running' || runtime.attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped')) throw new Conflict('Interrupted launch is not safely stopped.');
      for (const assignment of runtime.assignments) {
        const old = this.getAttempt(assignment.currentAttemptId);
        if (old.state === 'admitted') continue;
        const attempt: Attempt = { id: randomUUID(), assignmentId: assignment.id, runId, generation: old.generation + 1, input: old.input, state: 'admitted', createdAt: new Date(this.clock()).toISOString() };
        this.db.prepare('INSERT INTO attempts VALUES(?,?,?)').run(attempt.id, assignment.id, JSON.stringify(attempt));
        assignment.currentAttemptId = attempt.id;
        this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignment.id);
      }
      run.status = 'awaiting_runner'; run.pauseReason = undefined;
      this.saveRun(run);
      return run;
    });
  }

  reclaimCodingSlot(runId: string, reviewRegressed = false) {
    return this.transaction(() => {
      let run = this.getRun(runId); const holder = this.codingSlot();
      if ((run.purpose ?? 'coding') !== 'coding' || !run.slotRelease) return;
      if (holder && holder.id !== runId) throw new Conflict(`${holder.taskSnapshot.key} holds the coding slot (${this.slotReason(holder)}). Resume this run when it is free.`);
      if (holder?.id === runId) return;
      const unstopped = this.unstoppedCodingRun(runId);
      if (unstopped) throw new Conflict(`${unstopped.taskSnapshot.key} still has unconfirmed coding process ownership.`);
      if (reviewRegressed) run = this.changeRun(runId, { type: 'reviewRegressed' });
      run.schedule = { id: randomUUID(), at: new Date(this.clock()).toISOString() };
      // Review releases retain their candidate; a migration release has no published revision to guard.
      if (run.slotRelease?.reason === 'migration') run.slotRelease = undefined;
      this.saveRun(run);
      this.db.prepare('UPDATE coding_slot SET run_id=? WHERE id=1').run(runId);
    });
  }
  /** Persist the selected PR feedback and the queued control atomically, even when the coding slot is free. */
  queueFeedbackResume(runId: string): boolean {
    return this.transaction(() => {
      const run = this.getRun(runId); const publication = this.publication(runId);
      const previous = run.feedbackContinuations?.at(-1);
      if (run.feedbackResumeHold?.terminal) throw new FeedbackResumeConflict(run.feedbackResumeHold.reason, true);
      if (previous?.state === 'blocked') throw new FeedbackResumeConflict(previous.holdReason ?? 'This feedback continuation is terminal.', true);
      if (!publication?.feedback?.comments.length || publication.pr.state !== 'confirmed' || !publication.pr.id) {
        if (run.resumeRequest && previous) throw new FeedbackResumeConflict('The captured PR feedback or delivery identity is unavailable. Retain it for inspection.', false);
        return false;
      }
      if (publication.merged || publication.pr.providerState !== 'OPEN')
        throw new FeedbackResumeConflict('This PR is closed, merged or its open state is unconfirmed; feedback continuation is blocked.',
          !!publication.merged || ['MERGED', 'DECLINED', 'SUPERSEDED'].includes(publication.pr.providerState ?? ''));
      if ((run.purpose ?? 'coding') !== 'coding' || !['paused', 'attention_required'].includes(run.status) || run.desiredState === 'cancelled')
        throw new Conflict('Feedback continuation requires a paused coding run.');
      const work = this.repositoryWork(runId);
      if (!work || work.branch !== publication.branch || work.repository.path !== publication.repositoryPath)
        throw new FeedbackResumeConflict('The owned branch or repository no longer matches the published PR.', true);
      delete run.feedbackResumeHold;
      if (run.resumeRequest) {
        const holder = this.codingSlot();
        return (!!holder && holder.id !== runId) || !!this.unstoppedCodingRun(runId);
      }
      const latest = this.codingRecord(runId)?.candidates.at(-1);
      if (latest && latest.id !== publication.candidateId) {
        const sameWork = previous && latest.feedbackFrom?.continuationId === previous.id && previous.publicationId === publication.id
          && (previous.state === 'candidate' && previous.candidateId === latest.id
            || ['queued', 'starting', 'handed_off', 'held'].includes(previous.state));
        if (!sameWork) throw new FeedbackResumeConflict('A newer local candidate is not the published PR source. Validate or reconcile it before selecting new feedback.', false);
        if (previous.state === 'held') { previous.state = 'handed_off'; previous.holdReason = undefined; }
        run.resumeRequest = { requestedAt: new Date(this.clock()).toISOString(), state: 'queued' };
        this.saveRun(run); return true;
      }
      if (previous && ['queued', 'starting', 'handed_off', 'held'].includes(previous.state)) {
        if (previous.publicationId !== publication.id || previous.prId !== publication.pr.id || previous.branch !== publication.branch)
          throw new FeedbackResumeConflict('The captured PR identity changed; this continuation cannot be reused.', true);
        if (previous.state === 'held') { previous.state = 'queued'; previous.holdReason = undefined; }
        run.resumeRequest = { requestedAt: new Date(this.clock()).toISOString(), state: 'queued' };
        this.saveRun(run); return true;
      }
      const versions = (feedback: FeedbackObservation) => JSON.stringify(feedback.comments
        .map(({ currentness: _currentness, ...comment }) => comment).sort((left, right) => left.id - right.id));
      const same = previous?.publicationId === publication.id && previous.prId === publication.pr.id
        && previous.sourceCandidateId === publication.candidateId && previous.sourceHead === publication.revision.head
        && versions(previous.feedback) === versions(publication.feedback);
      const requestedAt = new Date(this.clock()).toISOString();
      if (!same) run.feedbackContinuations = [...(run.feedbackContinuations ?? []), {
        id: randomUUID(), requestedAt, publicationId: publication.id, repository: publication.repository, prId: publication.pr.id,
        branch: publication.branch, sourceCandidateId: publication.candidateId, sourceHead: publication.revision.head,
        feedback: structuredClone(publication.feedback), state: 'queued' }];
      run.resumeRequest = { requestedAt, state: 'queued' };
      this.saveRun(run);
      return true;
    });
  }

  /** Resolve a queued control without discarding its captured feedback or leaving it ahead of the global queue. */
  holdQueuedFeedbackResume(runId: string, reason: string, terminal: boolean) {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (!run.resumeRequest) return run;
      const selected = run.feedbackContinuations?.at(-1);
      if (selected && ['queued', 'starting', 'handed_off', 'held'].includes(selected.state)) {
        selected.state = terminal ? 'blocked' : 'held'; selected.holdReason = reason;
      }
      run.feedbackResumeHold = { reason, terminal };
      delete run.resumeRequest; this.saveRun(run);
      const held = this.changeRun(runId, { type: 'attention', reason });
      if (terminal && this.codingSlot()?.id === runId && !this.repositoryWork(runId)?.writer
        && this.runtime(runId).attempts.every(item => item.state === 'admitted' || item.ownership === 'stopped'))
        this.db.prepare('UPDATE coding_slot SET run_id=NULL WHERE id=1').run();
      return held;
    });
  }

  /** Persist explicit Resume while another run holds the slot or still has unconfirmed coding ownership. */
  queueReviewResume(runId: string): boolean {
    return this.transaction(() => {
      const run = this.getRun(runId); const holder = this.codingSlot();
      if ((!holder || holder.id === runId) && !this.unstoppedCodingRun(runId)) return false;
      if ((run.purpose ?? 'coding') !== 'coding' || run.slotRelease?.reason !== 'review') return false;
      if (!['paused', 'attention_required'].includes(run.status) || run.desiredState === 'cancelled') throw new Conflict('This review continuation is no longer available.');
      if (!run.resumeRequest) {
        run.resumeRequest = { requestedAt: new Date(this.clock()).toISOString(), state: 'queued' };
        this.saveRun(run);
      }
      return true;
    });
  }

  setResumeRequestState(runId: string, state: 'queued' | 'starting') {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (!run.resumeRequest || run.resumeRequest.state === state) return run;
      run.resumeRequest = { ...run.resumeRequest, state };
      if (state === 'starting') {
        const current = run.feedbackContinuations?.at(-1);
        if (current?.state === 'queued') current.state = 'starting';
      }
      return this.saveRun(run);
    });
  }

  clearResumeRequest(runId: string, disposition: 'held' | 'cancelled' = 'held') {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (!run.resumeRequest) return run;
      const selected = run.feedbackContinuations?.at(-1);
      if (selected && ['queued', 'starting'].includes(selected.state)) selected.state = disposition;
      delete run.resumeRequest;
      return this.saveRun(run);
    });
  }
  markFeedbackHandoff(runId: string, id: string) {
    return this.transaction(() => {
      const run = this.getRun(runId); const selected = run.feedbackContinuations?.find(item => item.id === id);
      if (selected?.state === 'starting') { selected.state = 'handed_off'; this.saveRun(run); }
      return selected;
    });
  }
  finishFeedbackContinuation(runId: string, id: string, outcome: { candidateId: string } | { noChange: true }) {
    return this.transaction(() => {
      const run = this.getRun(runId); const selected = run.feedbackContinuations?.find(item => item.id === id);
      if (selected && ('candidateId' in outcome ? selected.state === 'candidate' && selected.candidateId === outcome.candidateId
        : selected.state === 'no_change')) return selected;
      if (selected?.state === 'candidate' && 'candidateId' in outcome) {
        const candidates = this.codingRecord(runId)?.candidates ?? [];
        const successor = candidates.at(-1);
        if (successor?.id === outcome.candidateId && successor.feedbackFrom?.continuationId === id
          && successor.feedbackFrom.sourceCandidateId === selected.candidateId) {
          selected.candidateId = successor.id; this.saveRun(run); return selected;
        }
      }
      if (!selected || !['starting', 'handed_off'].includes(selected.state)) throw new Conflict('The feedback continuation is not active.');
      selected.state = 'candidateId' in outcome ? 'candidate' : 'no_change';
      if ('candidateId' in outcome) selected.candidateId = outcome.candidateId;
      this.saveRun(run); return selected;
    });
  }



  private queueReplay<T>(requestId: string, kind: string, payload: unknown): T | undefined {
    const row = this.db.prepare('SELECT * FROM queue_requests WHERE request_id=?').get(requestId);
    if (!row) return undefined;
    if (row.kind !== kind || row.payload !== JSON.stringify(payload)) throw new Conflict('Queue requestId already has different content.');
    return JSON.parse(String(row.result)) as T;
  }

  continuationReplay(runId: string, input: unknown) {
    const value = continuationRequest.parse(input);
    return this.queueReplay<Run>(value.requestId, 'continue-later', { runId, ...value });
  }

  /** Walk only verified ownership handoffs; evidence itself always belongs to its original run. */
  continuationPublication(runId: string) {
    const own = this.publication(runId); if (own) return own;
    const visited = new Set<string>(); let child = this.getRun(runId);
    const identity = child.continuedFrom;
    if (!identity?.publicationId) return undefined;
    while (child.continuedFrom) {
      if (visited.has(child.id)) throw new Conflict('Continuation held: publication lineage contains a cycle.');
      visited.add(child.id);
      const link = child.continuedFrom; const parent = this.getRun(link.runId);
      if (link.publicationId !== identity.publicationId || link.prId !== identity.prId || parent.continuedBy !== child.id
        || this.repositoryWork(parent.id)?.transferredTo !== child.id)
        throw new Conflict('Continuation held: the predecessor publication or ownership no longer matches.');
      const record = this.publication(parent.id);
      if (record) {
        if (record.id !== identity.publicationId || record.pr.id !== identity.prId)
          throw new Conflict('Continuation held: the predecessor PR identity changed.');
        return record;
      }
      child = parent;
    }
    throw new Conflict('Continuation held: the predecessor publication is unavailable.');
  }

  publicationSuperseded(runId: string, publicationId: string) {
    const visited = new Set<string>(); let parent = this.getRun(runId);
    while (parent.continuedBy && !visited.has(parent.id)) {
      visited.add(parent.id);
      const child = this.getRun(parent.continuedBy);
      if (child.continuedFrom?.runId !== parent.id || this.repositoryWork(parent.id)?.transferredTo !== child.id) return false;
      if (this.publication(child.id)?.id === publicationId) return true;
      parent = child;
    }
    return false;
  }

  /** Durable facts only; Git and the provider are also verified before queueing and admission. */
  cancelledSource(runId: string, successorId?: string) {
    const run = this.getRun(runId); const work = this.repositoryWork(runId); const publication = this.continuationPublication(runId);
    const refuse = (reason: string): never => { throw new Conflict(`Continuation held: ${reason}`); };
    if ((run.purpose ?? 'coding') !== 'coding' || run.status !== 'cancelled' || run.desiredState !== 'cancelled') refuse('Cancel must be confirmed before continuing later.');
    if (run.continuedBy && run.continuedBy !== successorId) refuse('This run already has a linked successor. Open it from run history.');
    if (!work || work.state !== 'ready' || !work.checkpoints.length) return refuse('No owned work is available. Queue a new-branch run instead.');
    if (work.transferredTo && work.transferredTo !== successorId) refuse('Worktree ownership belongs to another run.');
    if (work.writer || this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) refuse('Process or writer ownership is unresolved.');
    if (work.refreshes?.some(item => item.state === 'rebasing' || item.state === 'conflicted' && !item.supersededBy
      || item.resolutionWorkspace?.legacyOwnershipUnknown || ['preparing', 'continuing', 'promoting'].includes(item.resolutionWorkspace?.state ?? '')))
      refuse('Target recovery or Git ownership is unresolved. Inspect the retained workspace.');
    const coding = this.codingRecord(runId);
    if (coding?.edits?.some(item => item.state === 'pending') || coding?.completions?.some(item => item.state === 'requested')
      || coding?.checkOperations?.some(item => !['recorded', 'rejected'].includes(item.state))) refuse('A saved edit, completion or check still needs reconciliation.');
    if (publication && (publication.merged || publication.pr.providerState !== 'OPEN' || publication.pr.state !== 'confirmed'
      || !publication.pr.id || publication.push.state !== 'confirmed')) refuse('The PR is closed, merged or its delivery is unconfirmed. Reconcile publication before continuing.');
    const task = this.getTask(run.taskId); const configured = task.repositoryId && this.repositoryById(task.repositoryId);
    if (!configured || configured.retiredAt || configured.id !== work.repository.id || configured.path !== work.repository.path
      || configured.targetBranch !== work.repository.targetBranch) refuse('The selected repository no longer matches the retained branch.');
    return { run, work, publication, configured: configured as Repository, checkpoint: work.checkpoints.at(-1)! };
  }

  continuationView(runId: string) {
    const work = this.repositoryWork(runId);
    let publication: PublicationRecord | undefined;
    let reason: string | undefined;
    try { publication = this.continuationPublication(runId); this.cancelledSource(runId); } catch (error) { reason = error instanceof Error ? error.message : 'Continuation is unavailable.'; }
    return { branch: work?.branch, head: work?.checkpoints.at(-1)?.revision.head, prUrl: publication?.pr.url, reason };
  }

  queueCancelledContinuation(runId: string, input: unknown, roles: Role[], expectedWork: RepositoryWorkRecord): Run {
    const value = continuationRequest.parse(input);
    return this.transaction(() => {
      const replay = this.continuationReplay(runId, value); if (replay) return replay;
      const source = this.cancelledSource(runId); const task = this.getTask(source.run.taskId);
      if (JSON.stringify(source.work) !== JSON.stringify(expectedWork)) throw new Conflict('Continuation held: retained work changed during verification.');
      if (task.revision !== value.revision || task.needsReacceptance || !task.brief.trim() || !task.criteria.length)
        throw new Conflict('Accept the current brief and criteria before continuing.');
      if (this.openRun(task.id)) throw new Conflict('This ticket already has an open run.');
      const launch = parseLaunch({ team: [{ name: 'Coordinator', roleId: value.roleId, coordinator: true,
        input: JSON.stringify({ brief: task.brief, criteria: task.criteria, previousRunId: runId,
          continuation: 'Continue the retained branch and edits. Previous evidence is historical; collect fresh checks and independent review. If the target moved, record fresh implementation, then pause for an explicit target refresh before validation.',
          previousCompletions: this.codingRecord(runId)?.completions?.filter(item => item.state === 'completed').map(item => item.summary) ?? [] }) }],
        repository: { mode: 'continue', branch: source.work.branch, expectedHead: source.checkpoint.revision.head } }, roles);
      this.validateLaunchBriefs(task, launch);
      const run = this.createRun(task, 'coding', source.run.activeLimitMs ?? 3600000, value.requestId);
      run.launch = launch;
      run.continuedFrom = { runId, branch: source.work.branch, expectedHead: source.checkpoint.revision.head,
        worktreeHash: source.checkpoint.revision.worktreeHash, ...(source.publication ? { publicationId: source.publication.id, prId: source.publication.pr.id } : {}) };
      this.saveRun(run); source.run.continuedBy = run.id; this.saveRun(source.run);
      this.recordQueueRequest(value.requestId, 'continue-later', { runId, ...value }, run);
      return run;
    });
  }

  transferCancelledWork(runId: string, expectedWork: RepositoryWorkRecord, repository: Repository, checkpoint: RepositoryCheckpoint, continuationTargetRef?: string) {
    return this.transaction(() => {
      const run = this.getRun(runId); const link = run.continuedFrom;
      if (!link || run.status !== 'awaiting_runner' || ['paused', 'cancelled'].includes(run.desiredState ?? '') || this.runtime(runId).attempts.some(item => item.state !== 'admitted'))
        throw new Conflict('Continuation held: a newer control or execution superseded worktree admission.');
      const source = this.cancelledSource(link.runId, runId);
      if (source.run.continuedBy !== runId || JSON.stringify(source.work) !== JSON.stringify(expectedWork)
        || JSON.stringify(this.repositoryById(repository.id)) !== JSON.stringify(repository)) throw new Conflict('Continuation held: retained ownership or repository configuration changed.');
      const previous = this.repositoryWork(runId); if (previous) return previous;
      const work: RepositoryWorkRecord = { ...source.work, runId, repository, mode: 'continue', startHead: link.expectedHead,
        requirementsHash: requirementsHash(run.taskSnapshot, run.adoptions?.length ? run.taskRevision : undefined), checkpoints: [checkpoint],
        writer: undefined, transferredTo: undefined, refreshes: undefined,
        ...(continuationTargetRef ? { targetRef: source.work.base, continuationTargetRef } : {}) };
      source.work.transferredTo = runId;
      this.saveRepositoryWork(source.work);
      this.db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(work.runId, work.commonDir, work.branch, JSON.stringify(work));
      return this.repositoryWork(runId)!;
    });
  }

  private recordQueueRequest(requestId: string, kind: string, payload: unknown, result: unknown) {
    this.db.prepare('INSERT INTO queue_requests VALUES(?,?,?,?)').run(requestId, kind, JSON.stringify(payload), JSON.stringify(result));
  }
  configuredCheckIds(runId: string): string[] {
    const run = this.getRun(runId);
    if ((run.purpose ?? 'coding') !== 'coding') return [];
    const work = this.repositoryWork(runId);
    if (work) return (work.repository.checks ?? []).map(check => check.id);
    const row = run.taskSnapshot.repositoryId && this.db.prepare('SELECT data FROM repositories WHERE id=?').get(run.taskSnapshot.repositoryId);
    return row ? ((JSON.parse(String(row.data)) as Repository).checks ?? []).map(check => check.id) : [];
  }

  private validateLaunchBriefs(task: Task, launch: LaunchSpec) {
    if (!task.repositoryId) throw new Conflict('Select a target repository before saving launch details.');
    const repository = this.repositoryById(task.repositoryId);
    if (repository.retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
    snapshotBriefs(launch.team, { criteria: task.criteria, checkIds: (repository.checks ?? []).map(check => check.id), existing: [] });
  }


  setQueueLaunch(runId: string, input: unknown, roles: Role[]): Run {
    const { requestId, launch: raw } = z.object({ requestId: z.string().uuid(), launch: z.unknown() }).strict().parse(input);
    const launch = parseLaunch(raw);
    const payload = { runId, launch };
    return this.transaction(() => {
      const replay = this.queueReplay<Run>(requestId, 'launch', payload);
      if (replay) return replay;
      const run = this.getRun(runId);
      if ((run.purpose ?? 'coding') !== 'coding' || run.position === undefined || this.runtime(runId).assignments.length) throw new Conflict('Launch details can change only on an unadmitted queued coding plan.');
      if (run.continuedFrom && (launch.repository.mode !== 'continue' || launch.repository.branch !== run.continuedFrom.branch
        || launch.repository.expectedHead !== run.continuedFrom.expectedHead)) throw new Conflict('A linked continuation must retain its selected branch and expected head.');
      parseLaunch(raw, roles);
      this.validateLaunchBriefs(run.taskSnapshot, launch);
      this.validateInsertionTeam(runId, launch.team);
      run.launch = launch;
      delete run.launchHold;
      this.saveRun(run);
      this.recordQueueRequest(requestId, 'launch', payload, run);
      return run;
    });
  }

  reorderQueue(input: unknown) {
    const value = z.object({ requestId: z.string().uuid(), expectedRevision: z.number().int().nonnegative(), runIds: z.array(z.string().uuid()) }).strict().parse(input);
    const payload = { expectedRevision: value.expectedRevision, runIds: value.runIds };
    return this.transaction(() => {
      const replay = this.queueReplay<CodingQueue>(value.requestId, 'order', payload);
      if (replay) return replay;
      const current = this.codingQueue();
      if (current.revision !== value.expectedRevision) throw new Conflict('Queue order changed. Reload before reordering.');
      if (current.plans.length !== value.runIds.length || new Set(value.runIds).size !== current.plans.length
        || value.runIds.some(id => !current.plans.some(plan => plan.id === id))) throw new Conflict('Queue order must be a permutation of all queued coding plans.');
      const positions = new Map(value.runIds.map((id, index) => [id, index]));
      for (const plan of current.plans) {
        plan.position = positions.get(plan.id)!;
        this.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(plan), plan.id);
      }
      this.bumpQueueRevision();
      const ordered = this.codingQueue();
      this.recordQueueRequest(value.requestId, 'order', payload, ordered);
      return ordered;
    });
  }

  cancelQueued(runId: string, input: unknown): Run {
    const { requestId } = z.object({ requestId: z.string().uuid() }).strict().parse(input);
    return this.transaction(() => {
      const replay = this.queueReplay<Run>(requestId, 'cancel', { runId });
      if (replay) return replay;
      const run = this.getRun(runId);
      if ((run.purpose ?? 'coding') !== 'coding') throw new Conflict('Only coding plans use the queue.');
      if (run.status === 'cancelled') { this.recordQueueRequest(requestId, 'cancel', { runId }, run); return run; }
      if (run.position === undefined || this.runtime(runId).assignments.length) throw new Conflict('Only an unadmitted queued coding plan can be removed from the queue.');
      this.changeRun(runId, { type: 'desire', desiredState: 'cancelled' });
      this.changeRun(runId, { type: 'beginPause' });
      const cancelled = this.changeRun(runId, { type: 'finishPause', unresolved: false });
      this.recordQueueRequest(requestId, 'cancel', { runId }, cancelled);
      return this.getRun(runId);
    });
  }

  setDesiredState(runId: string, desiredState: NonNullable<Run['desiredState']>) {
    return this.transaction(() => {
      const run = this.changeRun(runId, { type: 'desire', desiredState });
      if (desiredState === 'cancelled') this.releaseCodingSlotInTransaction(runId, 'cancelled');
      return run;
    });
  }

  prepareContinuation(runId: string, controlRevision: number) {
    return this.transaction(() => {
      if (this.runtime(runId).assignments.some(assignment => !assignment.retiredAt && !assignment.rebriefRequired && this.getAttempt(assignment.currentAttemptId).state !== 'admitted')) throw new Conflict('An existing attempt still owns this assignment.');
      return this.changeRun(runId, { type: 'continue', controlRevision });
    });
  }

  private accountTimeInTransaction(runId: string) {
    const run=this.getRun(runId);
    let waiting=false;
    if (run.status==='running' && !run.sourceConflict && !run.codingTransition && !run.teamTransition && hostedCiPending(this.publication(runId))) {
      const runtime=this.runtime(runId);
      const current=runtime.assignments.filter(assignment=>!assignment.retiredAt).map(assignment=>runtime.attempts.find(attempt=>attempt.id===assignment.currentAttemptId)!);
      const commands=new Map<string, boolean>();
      for (const {attemptId,event} of this.commandEvents(runId)) {
        // Controller-confirmed ownership covers every tool, including a host killed before its exit event.
        if (runtime.attempts.find(attempt=>attempt.id===attemptId)?.ownership==='stopped') continue;
        const key=`${attemptId}:${event.toolCallId}`;
        if (['tool_started','tool_receipt_reused'].includes(event.type)) commands.set(key,true);
        if (event.type==='tool_exited' && event.ownership?.state==='stopped') commands.set(key,false);
      }
      waiting=current.length>0 && current.every(attempt=>attempt.ownership==='stopped' || (attempt.ownership==='held' && attempt.state==='idle' && this.hasAgentEvent(attempt.id,'session_settled')))
        && runtime.attempts.every(attempt=>attempt.ownership==='stopped' || attempt.state==='admitted' || current.some(item=>item.id===attempt.id && item.ownership==='held' && item.state==='idle')) && ![...commands.values()].some(Boolean)
        && runtime.messages.every(message=>message.disposition==='received')
        && !this.steering(runId).some(message=>['pending','dispatched'].includes(message.delivery))
        && !this.deliveryNotices(runId).some(notice=>notice.disposition!=='received')
        && !this.teamProposals(runId).some(proposal=>proposal.status==='pending');
    }
    return this.changeRun(runId,{type:'account',waiting});
  }

  accountTime(runId: string) { return this.transaction(() => this.accountTimeInTransaction(runId)); }

  requireAttention(runId: string, reason: string) {
    return this.transaction(() => this.changeRun(runId, { type: 'attention', reason }));
  }

  startExecution(runId: string) {
    return this.transaction(() => this.changeRun(runId, { type: 'start' }));
  }

  setActiveLimit(runId: string, input: unknown) {
    const { activeLimitMs } = z.object({ activeLimitMs: z.number().int().min(1000).max(Number.MAX_SAFE_INTEGER) }).strict().parse(input);
    return this.transaction(() => this.changeRun(runId, { type: 'setLimit', activeLimitMs }));
  }

  beginPause(runId: string, reason?: Run['pauseReason']) {
    return this.transaction(() => this.changeRun(runId, { type: 'beginPause', reason }));
  }

  recordOwnership(attemptId: string, stopped: boolean, reason?: string) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      if (stopped && attempt.ownership !== 'stopped') attempt.stoppedAt = new Date(this.clock()).toISOString();
      attempt.ownership = stopped ? 'stopped' : 'uncertain';
      attempt.state = stopped ? 'exited' : 'attention_required';
      if (reason && attempt.ownershipReason !== reason) attempt.ownershipNotes = [...(attempt.ownershipNotes ?? []), { at: new Date(this.clock()).toISOString(), reason }];
      attempt.ownershipReason = reason;
      this.db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt), attempt.id);
      this.accountTimeInTransaction(attempt.runId);
    });
  }

  finishPause(runId: string) {
    return this.transaction(() => {
      const unresolved = this.runtime(runId).attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped');
      return this.changeRun(runId, { type: 'finishPause', unresolved });
    });
  }

  agentEvents(runId: string) {
    return this.db.prepare('SELECT e.attempt_id, e.sequence, e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? ORDER BY e.rowid')
      .all(runId).map(row => ({ attemptId: String(row.attempt_id), sequence: Number(row.sequence), event: JSON.parse(String(row.data)) }));
  }
  agentEventsOfType(runId: string, types: string[]) {
    return this.db.prepare(`SELECT e.attempt_id, e.sequence, e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? AND json_extract(e.data,'$.type') IN (${types.map(() => '?').join(',')}) ORDER BY e.rowid`)
      .all(runId, ...types).map(row => ({ attemptId: String(row.attempt_id), sequence: Number(row.sequence), event: JSON.parse(String(row.data)) }));
  }
  /** The attempt whose host runs in a tmux pane, preferring the newest attempt when ids are reused by a restarted server. */
  attemptForPane(pane: string) {
    const row = this.db.prepare("SELECT id FROM attempts WHERE json_extract(data,'$.pane')=? ORDER BY rowid DESC LIMIT 1").get(pane);
    return row ? String(row.id) : undefined;
  }
  /** Container command starts and exits for a run: the source of live check tiles. */
  commandEvents(runId: string) {
    return this.db.prepare("SELECT e.attempt_id, e.sequence, e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? AND json_extract(e.data,'$.type') IN ('tool_started','tool_receipt_reused','tool_exited','tool_cancelled') ORDER BY e.rowid")
      .all(runId).map(row => ({ attemptId: String(row.attempt_id), sequence: Number(row.sequence), event: JSON.parse(String(row.data)) }));
  }
  agentEventPage(runId: string, after: number, limit = 100) {
    const rows = this.db.prepare('SELECT e.rowid AS cursor, e.attempt_id, e.sequence, e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? AND e.rowid>? ORDER BY e.rowid LIMIT ?')
      .all(runId, after, limit + 1);
    const events = rows.slice(0, limit).map(row => ({ cursor: Number(row.cursor), attemptId: String(row.attempt_id), sequence: Number(row.sequence), event: JSON.parse(String(row.data)) }));
    return { events, after: events.at(-1)?.cursor ?? after, hasMore: rows.length > limit };
  }

  hasAgentEvent(attemptId: string, type: string) {
    return !!this.db.prepare("SELECT 1 FROM agent_events WHERE attempt_id=? AND json_extract(data,'$.type')=? LIMIT 1").get(attemptId, type);
  }
  private compactCompletedMessage(attemptId: string, finalSequence: number, event: Record<string, unknown>) {
    if (event.type !== 'message_end' || !event.message || typeof event.message !== 'object') return;
    const message = event.message as Record<string, unknown>;
    const id = event.messageId ?? message.id;
    if (typeof id !== 'string' || !id || id.length > 200 ||
      (event.messageId !== undefined && message.id !== undefined && event.messageId !== message.id)) return;
    this.db.prepare("DELETE FROM agent_events WHERE attempt_id=? AND sequence<? AND json_extract(data,'$.type')='message_update' AND json_extract(data,'$.messageId')=?")
      .run(attemptId, finalSequence, id);
  }


  recordAgentEvent(attemptId: string, sequence: number, event: Record<string, unknown>) {
    z.number().int().positive().parse(sequence);
    event = compactObservation(event);
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      const saved = this.db.prepare('INSERT OR IGNORE INTO agent_events(attempt_id,sequence,data) VALUES(?,?,?)').run(attemptId, sequence, JSON.stringify(event));
      if (!saved.changes) return;
      if (attempt.ownership !== 'stopped') {
        if (!['failed', 'attention_required'].includes(attempt.state)) {
          if (event.type === 'agent_start') attempt.state = 'executing';
          if (event.type === 'session_settled') attempt.state = 'idle';
          if (event.type === 'process_exit') attempt.state = 'attention_required';
        }
        if (event.type === 'model_policy_rejected') {
          attempt.state = 'failed';
          this.changeRun(attempt.runId, { type: 'attention', reason: event.code === 'input_budget'
            ? 'Provider input budget is unavailable or exceeded. Inspect the retained budget observation before continuing.'
            : 'The provider attempted an unapproved model or reasoning setting.' });
        }
        if (event.type === 'prompt_result' && event.status === 'error') attempt.state = 'failed';
      }
      this.db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt), attemptId);
      if (event.type === 'agent_start') this.changeRun(attempt.runId, { type: 'agentStart' });
      if (['agent_start','session_settled','tool_started','tool_receipt_reused','tool_exited','process_exit','model_policy_rejected','prompt_result'].includes(String(event.type))) this.accountTimeInTransaction(attempt.runId);
      this.compactCompletedMessage(attemptId, sequence, event);
    });
  }

  recordObserverPage(assignmentId: string, events: { cursor: number; [key: string]: unknown }[], cursor: number) {
    return this.transaction(() => {
      const previous = this.observerCursor(assignmentId);
      if (!Number.isSafeInteger(cursor) || cursor < previous || events.some(event => !Number.isSafeInteger(event.cursor) || event.cursor > cursor)) throw new Conflict('Invalid observer cursor.');
      for (const event of events) this.db.prepare('INSERT OR IGNORE INTO observations VALUES(?,?,?)').run(assignmentId, event.cursor, JSON.stringify(event));
      this.db.prepare('INSERT INTO observer_cursors VALUES(?,?) ON CONFLICT(assignment_id) DO UPDATE SET cursor=excluded.cursor').run(assignmentId, cursor);
    });
  }

  private listeners = new Set<() => void>();
  /** Notifies observers after each committed change. Observers must not write back synchronously. */
  onChange(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }

  private transaction<T>(work: () => T): T {
    this.db.exec('BEGIN IMMEDIATE');
    let result: T;
    try { result = work(); this.db.exec('COMMIT'); }
    catch (error) { this.db.exec('ROLLBACK'); throw error; }
    for (const listener of this.listeners) { try { listener(); } catch { /* Observation never affects a committed change. */ } }
    return result;
  }

  private task(row: Record<string, unknown>): Task {
    return { ...JSON.parse(String(row.imported)), ...JSON.parse(String(row.refinement)) };
  }
  private allTasks(): Task[] { return this.db.prepare('SELECT * FROM tickets ORDER BY rowid').all().map(row => this.task(row)); }
  private allRuns(): Run[] { return this.db.prepare('SELECT data FROM runs ORDER BY rowid').all().map(row => JSON.parse(String(row.data)) as Run); }

  /** Replace the complete local dependency set with a keyed optimistic transaction. */
  setDependencies(taskId: string, input: unknown): Task {
    const value = z.object({ requestId: z.string().uuid(), dependencyRevision: z.number().int().nonnegative(), dependsOn: z.array(z.string().uuid()).max(12) }).strict().parse(input);
    const payload = { taskId, dependencyRevision: value.dependencyRevision, dependsOn: value.dependsOn };
    return this.transaction(() => {
      const replay = this.queueReplay<Task>(value.requestId, 'dependencies', payload);
      if (replay) return replay;
      const task = this.getTask(taskId);
      if ((task.dependencyRevision ?? 0) !== value.dependencyRevision) throw new Conflict('Ticket dependencies changed. Reload before editing dependencies.');
      if (new Set(value.dependsOn).size !== value.dependsOn.length) throw new Conflict('Dependencies must be distinct.');
      if (value.dependsOn.includes(taskId)) throw new Conflict('A ticket cannot depend on itself.');
      const tasks = new Map(this.allTasks().map(item => [item.id, item]));
      for (const id of value.dependsOn) if (!tasks.has(id)) throw new NotFound('Dependency ticket not found.');
      const visit = (id: string, seen: Set<string>): boolean => {
        if (id === taskId) return true;
        if (seen.has(id)) return false;
        seen.add(id);
        return (tasks.get(id)?.dependsOn ?? []).some(next => visit(next, seen));
      };
      if (value.dependsOn.some(id => visit(id, new Set()))) throw new Conflict('Ticket dependencies would form a cycle.');
      const row = this.db.prepare('SELECT refinement FROM tickets WHERE id=?').get(taskId)!;
      this.db.prepare('UPDATE tickets SET refinement=? WHERE id=?').run(JSON.stringify({
        ...JSON.parse(String(row.refinement)), dependsOn: value.dependsOn, dependencyRevision: value.dependencyRevision + 1,
      }), taskId);
      const saved = this.getTask(taskId);
      this.recordQueueRequest(value.requestId, 'dependencies', payload, saved);
      return saved;
    });
  }

  getTask(id: string): Task {
    const row = this.db.prepare('SELECT * FROM tickets WHERE id=?').get(id);
    if (!row) throw new NotFound('Ticket not found.');
    return this.task(row);
  }

  /** Accepted saves only; Jira imports never mutate locally accepted revisions. */
  requirementRevisions(taskId: string): RequirementRevision[] {
    this.getTask(taskId);
    return this.db.prepare('SELECT data FROM requirement_revisions WHERE task_id=? ORDER BY revision').all(taskId)
      .map(row => JSON.parse(String(row.data)) as RequirementRevision);
  }

  snapshot() {
    const sourceOrder = new Map((this.jiraState()?.taskIds ?? []).map((id, index) => [id, index]));
    const tasks = this.allTasks(); const publications = this.publications(); const holderId = this.codingSlot()?.id;
    const publicationByRun = new Map(publications.map(publication => [publication.runId, publication]));
    const runs = this.allRuns().map(run => {
      const readinessHold = this.readinessHold(run, holderId, publicationByRun.get(run.id));
      return readinessHold ? { ...run, readinessHold } : run;
    });
    return {
      tasks: tasks.sort((a, b) => {
        return (sourceOrder.get(a.id) ?? sourceOrder.size) - (sourceOrder.get(b.id) ?? sourceOrder.size);
      }),
      requirementRevisions: this.db.prepare('SELECT data FROM requirement_revisions ORDER BY rowid').all().map(row => JSON.parse(String(row.data)) as RequirementRevision),
      repositories: this.db.prepare('SELECT data FROM repositories ORDER BY rowid').all().map(row => JSON.parse(String(row.data)) as Repository),
      runs,
      repositorySnapshots: Object.fromEntries(this.db.prepare("SELECT run_id, json_extract(data,'$.repository') AS repository FROM repository_work").all()
        .map(row => [String(row.run_id), JSON.parse(String(row.repository)) as Repository])),
      continuations: Object.fromEntries(runs.filter(run => run.desiredState === 'cancelled' && (run.purpose ?? 'coding') === 'coding')
        .map(run => [run.id, this.continuationView(run.id)])),
      queue: this.codingQueue(),
      dependencyStates: Object.fromEntries(tasks.map(task => {
        const { state, reason } = dependencyDelivery(task, runs, publications);
        return [task.id, { state, reason }];
      })),
      notes: this.db.prepare('SELECT data FROM notes ORDER BY rowid').all().map(row => JSON.parse(String(row.data)) as Note),
      refinementProposals: this.refinementProposals(),
      messages: this.db.prepare('SELECT data FROM steering ORDER BY rowid').all().map(row => JSON.parse(String(row.data)) as Message),
      learning: this.learningProposals(),
    };
  }

  assignRepository(taskId: string, input: unknown): Task {
    const value = z.object({ repositoryId: z.string(), revision: z.number().int().positive() }).parse(input);
    return this.transaction(() => {
      const task = this.getTask(taskId);
      if (task.revision !== value.revision) throw new Conflict('The ticket changed. Reload before saving.');
      if (this.repositoryById(value.repositoryId).retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
      // Historical run snapshots keep their original repository; only an open run fixes the next target.
      if (this.openRun(taskId)) throw new Conflict('The repository is fixed for the admitted run.');
      const row = this.db.prepare('SELECT refinement FROM tickets WHERE id=?').get(taskId)!;
      this.db.prepare('UPDATE tickets SET refinement=? WHERE id=?').run(JSON.stringify({
        ...JSON.parse(String(row.refinement)), repositoryId: value.repositoryId, revision: task.revision + 1,
      }), taskId);
      return this.getTask(taskId);
    });
  }

  addNote(taskId: string, input: unknown): Note {
    this.getTask(taskId);
    const value = z.object({ mode: z.enum(['explore', 'grill', 'decision']), body: z.string().trim().min(1).max(40000) }).strict().parse(input);
    return this.persistNote(taskId, value);
  }

  private persistNote(taskId: string, value: Pick<Note, 'mode' | 'body' | 'proposalId' | 'revision' | 'kind'>): Note {
    const note: Note = { ...value, id: randomUUID(), taskId, createdAt: new Date(this.clock()).toISOString() };
    this.db.prepare('INSERT INTO notes VALUES(?,?,?)').run(note.id, taskId, JSON.stringify(note));
    return note;
  }

  refinementProposals(runId?: string): RefinementProposal[] {
    const rows = runId ? this.db.prepare('SELECT data FROM refinement_proposals WHERE run_id=? ORDER BY rowid').all(runId)
      : this.db.prepare('SELECT data FROM refinement_proposals ORDER BY rowid').all();
    return rows.map(row => JSON.parse(String(row.data)) as RefinementProposal);
  }

  proposeRefinement(attemptId: string, toolCallId: string, input: unknown): RefinementProposal {
    const value = proposalSchema.parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId); const run = this.getRun(attempt.runId);
      const existing = this.db.prepare('SELECT data FROM refinement_proposals WHERE attempt_id=? AND tool_call_id=?').get(attemptId, toolCallId);
      if (existing) {
        const saved = JSON.parse(String(existing.data)) as RefinementProposal;
        if (JSON.stringify([saved.brief, saved.criteria, saved.decisions, saved.unresolvedQuestions]) !== JSON.stringify([value.brief, value.criteria, value.decisions, value.unresolvedQuestions])) throw new Conflict('Proposal identity has different content.');
        return saved;
      }
      if (!['explore', 'grill'].includes(run.purpose ?? '') || run.status !== 'running' || run.desiredState !== 'running' || attempt.ownership !== 'held'
        || !this.runtime(run.id).assignments.some(item => item.coordinator && item.currentAttemptId === attemptId)) throw new Conflict('Only the active refinement coordinator may propose requirements.');
      const proposal: RefinementProposal = { ...value, id: randomUUID(), runId: run.id, taskId: run.taskId, attemptId, toolCallId,
        baseRevision: run.taskRevision, sourceRevision: run.taskSnapshot.sourceRevision ?? 1, status: 'pending', createdAt: new Date(this.clock()).toISOString() };
      this.db.prepare('INSERT INTO refinement_proposals VALUES(?,?,?,?,?)').run(proposal.id, run.id, attemptId, toolCallId, JSON.stringify(proposal));
      for (const body of proposal.decisions) this.persistNote(run.taskId, { mode: 'decision', body, kind: 'decision', proposalId: proposal.id, revision: proposal.baseRevision });
      for (const body of proposal.unresolvedQuestions) this.persistNote(run.taskId, { mode: run.purpose as 'explore' | 'grill', body, kind: 'unresolved_question', proposalId: proposal.id, revision: proposal.baseRevision });
      return proposal;
    });
  }

  decideRefinement(id: string, input: unknown): RefinementProposal {
    const value = z.object({ decision: z.enum(['accept', 'discard']), requestId: z.string().uuid(), reason: z.string().trim().min(1).max(4000).optional(),
      brief: z.string().trim().min(1).max(40000).optional(), criteria: proposalSchema.shape.criteria.optional() }).strict().parse(input);
    if (value.decision === 'discard' && (!value.reason || value.brief !== undefined || value.criteria !== undefined)) throw new Conflict('Discard requires a reason and cannot edit the proposal.');
    return this.transaction(() => {
      const row = this.db.prepare('SELECT data FROM refinement_proposals WHERE id=?').get(id);
      if (!row) throw new NotFound('Refinement proposal not found.');
      const proposal = JSON.parse(String(row.data)) as RefinementProposal;
      if (proposal.status !== 'pending') {
        if (proposal.status === (value.decision === 'accept' ? 'accepted' : 'discarded') && proposal.decision?.requestId === value.requestId) {
          if (JSON.stringify([proposal.decision.reason, proposal.decision.brief, proposal.decision.criteria]) !== JSON.stringify([value.reason, value.brief, value.criteria])) throw new Conflict('Decision identity has different content.');
          return proposal;
        }
        throw new Conflict('This proposal has already been decided.');
      }
      const task = this.getTask(proposal.taskId);
      if (value.decision === 'accept') {
        if (task.revision !== proposal.baseRevision || task.sourceRevision !== proposal.sourceRevision || task.needsReacceptance) throw new Conflict('The proposal is based on an earlier revision. Reload before accepting.');
        const accepted = this.saveAcceptedRequirements(task.id, { revision: task.revision, sourceRevision: task.sourceRevision ?? 1,
          brief: value.brief ?? proposal.brief, criteria: value.criteria ?? proposal.criteria }, { origin: 'refinement', proposalId: proposal.id });
        for (const row of this.db.prepare("SELECT id, data FROM notes WHERE json_extract(data,'$.proposalId')=?").all(id)) {
          const note = JSON.parse(String(row.data)) as Note;
          note.revision = accepted.revision;
          this.db.prepare('UPDATE notes SET data=? WHERE id=?').run(JSON.stringify(note), row.id);
        }
        proposal.status = 'accepted'; proposal.decision = { requestId: value.requestId, at: new Date(this.clock()).toISOString(), reason: value.reason, revision: accepted.revision,
          brief: value.brief, criteria: value.criteria };
      } else {
        proposal.status = 'discarded'; proposal.decision = { requestId: value.requestId, at: new Date(this.clock()).toISOString(), reason: value.reason, revision: proposal.baseRevision };
        this.persistNote(task.id, { mode: 'decision', body: value.reason!, proposalId: id, revision: proposal.baseRevision });
      }
      this.db.prepare('UPDATE refinement_proposals SET data=? WHERE id=?').run(JSON.stringify(proposal), id);
      return proposal;
    });
  }

  researchArtifacts(runId: string): ResearchArtifact[] {
    this.getRun(runId);
    return this.db.prepare('SELECT data FROM research_artifacts WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)) as ResearchArtifact);
  }

  saveResearchArtifact(artifact: ResearchArtifact): ResearchArtifact {
    return this.transaction(() => {
      const attempt = this.getAttempt(artifact.attemptId); const run = this.getRun(artifact.runId);
      const prior = this.db.prepare('SELECT data FROM research_artifacts WHERE attempt_id=? AND tool_call_id=?').get(artifact.attemptId, artifact.toolCallId);
      if (prior) return JSON.parse(String(prior.data)) as ResearchArtifact;
      const assignment = this.runtime(run.id).assignments.find(item => item.id === attempt.assignmentId && item.currentAttemptId === attempt.id && !item.retiredAt);
      if (run.purpose !== 'research' || run.status !== 'running' || run.desiredState !== 'running' || run.sourceConflict || run.researchCompletion || run.researchOutcome?.revision === run.taskRevision || !assignment || attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state)) throw new Conflict('Only a current research agent may submit an artifact before completion is requested.');
      if (artifact.requirementRevision !== run.taskRevision || artifact.pinnedCommit !== run.researchSource?.commit) throw new Conflict('Research requirements or source changed during submission.');
      const previous = this.researchArtifacts(run.id).findLast(item => item.assignmentId === artifact.assignmentId && item.title === artifact.title && item.kind === artifact.kind && !item.supersededBy);
      if (previous) {
        previous.supersededBy = artifact.id;
        this.db.prepare('UPDATE research_artifacts SET data=? WHERE id=?').run(JSON.stringify(previous), previous.id);
      }
      this.db.prepare('INSERT INTO research_artifacts VALUES(?,?,?,?,?)').run(artifact.id, run.id, attempt.id, artifact.toolCallId, JSON.stringify(artifact));
      return artifact;
    });
  }

  requestResearchCompletion(runId: string, attemptId: string, toolCallId: string) {
    return this.transaction(() => {
      const run = this.getRun(runId);
      const requestId = `${attemptId}:${toolCallId}`;
      if (run.researchCompletion) {
        if (run.researchCompletion.requestId !== requestId) throw new Conflict('Research completion has already been requested.');
        return run.researchCompletion;
      }
      if (run.purpose !== 'research' || run.status !== 'running' || run.desiredState !== 'running' || !this.runtime(runId).assignments.some(item => item.coordinator && item.currentAttemptId === attemptId)) throw new Conflict('Only the active research coordinator may request completion.');
      run.researchCompletion = { requestId, revision: run.taskRevision };
      this.saveRun(run);
      return run.researchCompletion;
    });
  }

  completeResearch(runId: string, requestId: string, revision: number): Run {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.status === 'completed' && run.researchOutcome?.requestId === requestId) return run;
      if (run.purpose !== 'research' || run.taskRevision !== revision || run.researchCompletion?.requestId !== requestId) throw new Conflict('Current accepted research evidence is required.');
      if (this.insertionBlockers(runId).length) throw new Conflict('Reconcile inserted specialist responsibilities before completing research.');
      const current = this.researchArtifacts(runId).filter(item => item.state === 'verified' && !item.supersededBy && item.requirementRevision === revision && item.pinnedCommit === run.researchSource?.commit && item.citations.every(citation => citation.verified));
      if (!run.taskSnapshot.brief.trim() || !run.taskSnapshot.criteria.length || !current.length || run.taskSnapshot.criteria.some((_, id) => !current.some(item => item.coverage.some(entry => entry.criterionId === id)))) throw new Conflict('Verified current research artifacts must cover every accepted criterion.');
      if (this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Research process ownership must be stopped.');
      const updated = this.changeRun(runId, { type: 'completeResearch' });
      updated.researchOutcome = { revision, completedAt: new Date(this.clock()).toISOString(), requestId };
      return this.saveRun(updated);
    });
  }

  closeRefinement(runId: string): Run {
    return this.transaction(() => {
      const run = this.getRun(runId);
      if (run.status === 'closed') return run;
      if (this.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Process ownership must be confirmed stopped before closing.');
      return this.changeRun(runId, { type: 'close' });
    });
  }

  steer(runId: string, input: unknown): Message {
    const value = z.object({ requestId: z.string().min(1).max(200), body: z.string().trim().min(1).max(40000) }).parse(input);
    return this.transaction(() => {
      const existing = this.db.prepare('SELECT data FROM steering WHERE run_id=? AND request_id=?').get(runId, value.requestId);
      if (existing) {
        const message = JSON.parse(String(existing.data)) as Message;
        if (message.body !== value.body) throw new Conflict('This steering request already has different content.');
        return message;
      }
      const reason = steeringClosedReason(this.getRun(runId));
      if (reason) throw new Conflict(`${reason} Steering cannot schedule more work.`);
      if (this.steering(runId).filter(message => ['pending', 'dispatched'].includes(message.delivery)).length >= pendingMessageLimit)
        throw new Conflict(`Coordinator backlog reached ${pendingMessageLimit} pending messages. Wait for receipt before sending more; this instruction was not saved.`);
      const message: Message = { id: randomUUID(), runId, body: value.body, createdAt: new Date(this.clock()).toISOString(), delivery: 'pending', transitions: [] };
      this.db.prepare('INSERT INTO steering VALUES(?,?,?,?)').run(message.id, runId, value.requestId, JSON.stringify(message));
      return message;
    });
  }

  steering(runId: string): Message[] {
    return this.db.prepare('SELECT data FROM steering WHERE run_id=? ORDER BY rowid').all(runId).map(row => JSON.parse(String(row.data)));
  }

  private transitionSteering(message: Message, state: Message['delivery'], reason?: string, at: string | undefined = new Date(this.clock()).toISOString()) {
    message.transitions = [...steeringTransitions(message), { state, ...(at ? { at } : {}), ...(reason ? { reason } : {}) }];
    message.delivery = state;
    if (state === 'not_delivered') message.dispositionReason = reason;
    this.db.prepare('UPDATE steering SET data=? WHERE id=?').run(JSON.stringify(message), message.id);
  }

  private settleUndeliveredSteering(run: Run, at?: string) {
    const reason = steeringUndeliveredReason(run);
    if (!reason) return;
    for (const message of this.steering(run.id)) if (message.delivery === 'pending') {
      // The migration deliberately records an unknown time, not the time of reopening.
      message.transitions = [...steeringTransitions(message), { state: 'not_delivered', ...(at ? { at } : {}), reason }];
      message.delivery = 'not_delivered'; message.dispositionReason = reason;
      this.db.prepare('UPDATE steering SET data=? WHERE id=?').run(JSON.stringify(message), message.id);
    }
  }

  dispatchSteering(attemptId: string, ids: string[], inputId: string) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      if (!this.runtime(attempt.runId).assignments.some(item => item.coordinator && item.currentAttemptId === attemptId)) throw new Conflict('Steering is delivered only to the current coordinator.');
      const reason = steeringClosedReason(this.getRun(attempt.runId));
      if (reason) throw new Conflict(`${reason} Steering cannot be dispatched.`);
      for (const id of ids) {
        const message = this.steering(attempt.runId).find(message => message.id === id);
        if (!message || message.delivery !== 'pending') throw new Conflict('Steering is not pending.');
        Object.assign(message, { attemptId, inputId });
        this.transitionSteering(message, 'dispatched');
      }
    });
  }

  receiveSteering(attemptId: string, inputId: string) {
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      for (const message of this.steering(attempt.runId)) if (message.delivery === 'dispatched' && message.attemptId === attemptId && message.inputId === inputId) {
        this.transitionSteering(message, 'received');
      }
    });
  }

  applySteering(attemptId: string, toolCallId: string, input: unknown) {
    const value = z.object({ messageId: z.string(), disposition: z.enum(['applied', 'rejected']), reason: z.string().trim().min(1).max(4000) }).strict().parse(input);
    return this.transaction(() => {
      const attempt = this.getAttempt(attemptId);
      if (!this.runtime(attempt.runId).assignments.some(item => item.coordinator && item.currentAttemptId === attemptId) || attempt.ownership !== 'held') throw new Conflict('Only the active coordinator can reconcile steering.');
      const message = this.steering(attempt.runId).find(message => message.id === value.messageId);
      if (!message || ['pending', 'not_delivered'].includes(message.delivery)) throw new Conflict('Steering has not been delivered to this run.');
      if (['applied', 'rejected'].includes(message.delivery)) {
        if (message.delivery !== value.disposition || message.dispositionReason !== value.reason) throw new Conflict('Steering already has a different disposition.');
        return message;
      }
      // An authenticated coordinator disposition also proves receipt when the host reply was lost.
      if (message.delivery === 'dispatched') this.transitionSteering(message, 'received');
      Object.assign(message, { dispositionReason: value.reason, dispositionToolCallId: toolCallId });
      this.transitionSteering(message, value.disposition, value.reason);
      return message;
    });
  }

  private repositoryById(id: string): Repository {
    const row = this.db.prepare('SELECT data FROM repositories WHERE id=?').get(id);
    if (!row) throw new NotFound('Repository not found.');
    return JSON.parse(String(row.data)) as Repository;
  }

  /** Open runs include queued plans, paused work and unscheduled active attempts. */
  private referencingRuns(repositoryId: string): Run[] {
    return this.db.prepare("SELECT data FROM runs WHERE status NOT IN ('cancelled','closed','completed') AND json_extract(data,'$.taskSnapshot.repositoryId')=?")
      .all(repositoryId).map(row => JSON.parse(String(row.data)) as Run);
  }

  private refuseReferenced(repositoryId: string, action: string, references = this.referencingRuns(repositoryId)) {
    if (references.length) throw new Conflict(`Cannot ${action} while queued or open work references this repository: ${[...new Set(references.map(run => run.taskSnapshot.key))].join(', ')}.`);
  }

  repositoryUpdateReplay(id: string, input: unknown): Repository | undefined {
    const value = repositoryUpdateSchema.parse(input);
    return this.queueReplay<Repository>(value.requestId, 'repository-update', { id, value });
  }

  updateRepository(id: string, input: ValidatedRepository, update: unknown): Repository {
    const value = repositoryUpdateSchema.parse(update);
    const validated = repositorySchema.parse(input);
    return this.transaction(() => {
      const payload = { id, value };
      const replay = this.queueReplay<Repository>(value.requestId, 'repository-update', payload);
      if (replay) return replay;
      const current = this.repositoryById(id);
      if (current.retiredAt) throw new Conflict('Retired repositories cannot be edited.');
      if ((current.revision ?? 1) !== value.revision) throw new Conflict('Repository revision changed. Keep your draft and reload the current configuration before saving.');
      if (this.isOwnedRepositoryWorktree(validated.path)) throw new Conflict('A Crewroom-owned linked worktree cannot be configured as a repository.');
      const others = this.db.prepare('SELECT data FROM repositories WHERE id<>?').all(id).map(row => JSON.parse(String(row.data)) as Repository);
      if (others.some(item => {
        if (item.retiredAt) return false;
        let path = item.path;
        try { path = realpathSync(item.path); } catch { /* Missing historical checkouts retain their saved path. */ }
        return path === validated.path && item.targetBranch === validated.targetBranch;
      })) throw new Conflict('This repository path and target branch are already configured.');
      const references = this.referencingRuns(id);
      const removed = new Set((current.checks ?? []).map(check => check.id).filter(checkId => !(validated.checks ?? []).some(check => check.id === checkId)));
      const referencedCheck = references.some(run => run.launch?.team.some(member => member.criteria.some(item => item.checkIds.some(checkId => removed.has(checkId)))));
      if (current.path !== validated.path || current.targetBranch !== validated.targetBranch || referencedCheck)
        this.refuseReferenced(id, 'change checkout, target branch or referenced checks', references);
      const { requestId: _requestId, ...fields } = validated;
      const saved: Repository = { id: current.id, ...(current.requestId ? { requestId: current.requestId } : {}),
        ...(current.guidance ? { guidance: current.guidance } : {}), ...fields, revision: (current.revision ?? 1) + 1 };
      this.db.prepare('UPDATE repositories SET data=? WHERE id=?').run(JSON.stringify(saved), id);
      this.recordQueueRequest(value.requestId, 'repository-update', payload, saved);
      return saved;
    });
  }

  retireRepository(id: string, input: unknown): Repository {
    const value = z.object({ revision: z.number().int().positive(), requestId: z.string().uuid() }).strict().parse(input);
    return this.transaction(() => {
      const payload = { id, value };
      const replay = this.queueReplay<Repository>(value.requestId, 'repository-retire', payload);
      if (replay) return replay;
      const current = this.repositoryById(id);
      if ((current.revision ?? 1) !== value.revision) throw new Conflict('Repository revision changed. Reload the current configuration before retiring.');
      if (current.retiredAt) throw new Conflict('Repository is already retired.');
      this.refuseReferenced(id, 'retire');
      const saved = { ...current, revision: (current.revision ?? 1) + 1, retiredAt: new Date(this.clock()).toISOString() };
      this.db.prepare('UPDATE repositories SET data=? WHERE id=?').run(JSON.stringify(saved), id);
      this.recordQueueRequest(value.requestId, 'repository-retire', payload, saved);
      return saved;
    });
  }

  /** A keyed replay is checked before Git so a removed target ref does not invalidate an earlier receipt. */
  repositoryReplay(input: unknown): Repository | undefined {
    const value = repositorySchema.parse(input);
    if (!value.requestId) return undefined;
    let canonical = value.path;
    try { canonical = realpathSync(canonical); } catch { /* Replays remain available after checkout removal. */ }
    const receipt = this.queueReplay<Repository>(value.requestId, 'repository-create', { ...value, path: canonical });
    if (receipt) return receipt;
    const row = this.db.prepare("SELECT data FROM repositories WHERE json_extract(data,'$.requestId')=?").get(value.requestId);
    if (!row) return undefined;
    const previous = JSON.parse(String(row.data)) as Repository;
    let path = value.path;
    try { path = realpathSync(path); } catch { /* An old request can replay after its checkout is removed. */ }
    const { id: _id, guidance: _guidance, revision: _revision, retiredAt: _retiredAt, ...original } = previous;
    if (JSON.stringify(original) !== JSON.stringify({ ...value, path })) throw new Conflict('Repository requestId was already used for different configuration.');
    return previous;
  }

  isOwnedRepositoryWorktree(path: string) {
    return this.db.prepare('SELECT data FROM repository_work').all().some(row => (JSON.parse(String(row.data)) as RepositoryWorkRecord).path === path);
  }

  /** Caller supplies the result of Git preflight; duplicate and replay decisions remain atomic. */
  saveRepository(input: ValidatedRepository): Repository {
    const value = repositorySchema.parse(input);
    return this.transaction(() => {
      const repositories = this.snapshot().repositories;
      if (value.requestId) {
        const replay = this.queueReplay<Repository>(value.requestId, 'repository-create', value);
        if (replay) return replay;
        const legacy = repositories.find(item => item.requestId === value.requestId);
        if (legacy) {
          const { id: _id, guidance: _guidance, revision: _revision, retiredAt: _retiredAt, ...original } = legacy;
          if (JSON.stringify(original) !== JSON.stringify(value)) throw new Conflict('Repository requestId was already used for different configuration.');
          return legacy;
        }
      }
      if (this.isOwnedRepositoryWorktree(value.path)) throw new Conflict('A Crewroom-owned linked worktree cannot be configured as a repository.');
      if (repositories.some(item => {
        if (item.retiredAt) return false;
        let configured = item.path;
        try { configured = realpathSync(item.path); } catch { /* A missing old checkout cannot resolve to the new checkout. */ }
        return configured === value.path && item.targetBranch === value.targetBranch;
      })) throw new Conflict('This repository path and target branch are already configured.');
      const repository = { ...value, id: randomUUID(), revision: 1 };
      this.db.prepare('INSERT INTO repositories VALUES(?,?)').run(repository.id, JSON.stringify(repository));
      if (value.requestId) this.recordQueueRequest(value.requestId, 'repository-create', value, repository);
      return repository;
    });
  }

  createTask(input: unknown): Task {
    const value = z.object({ title: z.string().trim().min(1).max(300), description: z.string().max(40000),
      repositoryId: z.string().optional() }).parse(input);
    return this.transaction(() => {
      if (value.repositoryId && this.repositoryById(value.repositoryId).retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
      const id = randomUUID();
      const source = { id, key: `LOCAL-${id.slice(0, 8)}`, title: value.title, description: value.description,
        jiraStatus: 'Not linked', priority: 'Unspecified', source: 'local', project: '', rank: 0,
        sourceRevision: 1, updatedAt: new Date().toISOString() };
      this.db.prepare('INSERT INTO tickets VALUES(?,?,?,?,?)').run(id, null, null, JSON.stringify(source), JSON.stringify({
        revision: 1, brief: '', criteria: [], needsReacceptance: false, repositoryId: value.repositoryId,
      }));
      return this.getTask(id);
    });
  }

  /** At most one open run is enforced by the partial SQLite unique index, including across connections. */
  private openRun(taskId: string): Run | undefined {
    const row = this.db.prepare("SELECT data FROM runs WHERE task_id=? AND status NOT IN ('cancelled','closed','completed')").get(taskId);
    return row ? JSON.parse(String(row.data)) as Run : undefined;
  }

  private createRun(task: Task, purpose: 'coding' | 'explore' | 'grill' | 'research', activeLimitMs: number, requestId?: string): Run {
    const run: Run = { id: randomUUID(), taskId: task.id, purpose, requestId, status: 'awaiting_runner',
      ...(purpose === 'coding' ? { position: this.codingQueue().plans.length } : {}),
      createdAt: new Date(this.clock()).toISOString(), taskRevision: task.revision,
      taskSnapshot: task, roles: [], stages: [], activeMs: 0, activeLimitMs };
    this.db.prepare('INSERT INTO runs VALUES(?,?,?)').run(run.id, task.id, JSON.stringify(run));
    if (purpose === 'coding') this.bumpQueueRevision();
    return run;
  }

  queueRefinement(taskId: string, input: unknown): Run {
    const value = z.object({ revision: z.number().int().positive(), purpose: z.enum(['explore', 'grill']), requestId: z.string().uuid().optional() }).strict().parse(input);
    return this.transaction(() => {
      const task = this.getTask(taskId);
      const row = value.requestId && this.db.prepare("SELECT data FROM runs WHERE task_id=? AND json_extract(data,'$.requestId')=?").get(taskId, value.requestId);
      const replay = row ? JSON.parse(String(row.data)) as Run : undefined;
      if (replay) {
        if (replay.purpose !== value.purpose || replay.taskRevision !== value.revision) throw new Conflict('Refinement request identity has different content.');
        return replay;
      }
      if (task.revision !== value.revision) throw new Conflict('The brief changed. Reload before refining.');
      if (this.openRun(taskId)) throw new Conflict('Close the open run before starting another.');
      return this.createRun(task, value.purpose, 3600000, value.requestId);
    });
  }

  queueResearch(taskId: string, input: unknown, source?: Run['researchSource']): Run {
    const value = z.object({ revision: z.number().int().positive(), requestId: z.string().uuid(), activeLimitMs: z.number().int().min(1000).max(Number.MAX_SAFE_INTEGER).default(3600000) }).strict().parse(input);
    return this.transaction(() => {
      const task = this.getTask(taskId);
      const row = this.db.prepare("SELECT data FROM runs WHERE task_id=? AND json_extract(data,'$.requestId')=?").get(taskId, value.requestId);
      if (row) {
        const prior = JSON.parse(String(row.data)) as Run;
        if (prior.purpose !== 'research' || prior.taskRevision !== value.revision || prior.activeLimitMs !== value.activeLimitMs) throw new Conflict('Research request identity has different content.');
        return prior;
      }
      if (task.revision !== value.revision || task.needsReacceptance || !task.brief.trim() || !task.criteria.length) throw new Conflict('Accept a current brief and criteria before queuing research.');
      if (this.openRun(taskId)) throw new Conflict('Close the open run before queuing research.');
      const repository = task.repositoryId ? this.repositoryById(task.repositoryId) : undefined;
      if (repository?.retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
      if (!!task.repositoryId !== !!source || source?.repositoryId !== task.repositoryId) throw new Conflict('Selected research repository must be pinned before admission.');
      if (repository && source) {
        let checkout: string;
        try { checkout = realpathSync(repository.path); }
        catch { throw new Conflict('Selected research repository checkout is unavailable. Restore it before queuing research.'); }
        if (source.repositoryRevision !== (repository.revision ?? 1) || source.path !== checkout)
          throw new Conflict('Selected research repository changed during Git preflight. Retry queuing research from the current configuration.');
      }
      const run = this.createRun(task, 'research', value.activeLimitMs, value.requestId);
      run.researchSource = source;
      return this.saveRun(run);
    });
  }

  queue(taskId: string, input: unknown, roles: Role[] = []): Run {
    const { revision, activeLimitMs, requestId, launch: rawLaunch } = z.object({ revision: z.number().int().positive(), activeLimitMs: z.number().int().min(1000).max(Number.MAX_SAFE_INTEGER).default(3600000), requestId: z.string().uuid().optional(), launch: z.unknown().optional() }).strict().parse(input);
    const launch = rawLaunch === undefined ? undefined : parseLaunch(rawLaunch);
    const payload = { taskId, revision, activeLimitMs, launch };
    return this.transaction(() => {
      const replay = requestId && this.queueReplay<Run>(requestId, 'queue', payload);
      if (replay) return replay;
      const task = this.getTask(taskId);
      const row = requestId && this.db.prepare("SELECT data FROM runs WHERE json_extract(data,'$.requestId')=?").get(requestId);
      if (row) {
        const prior = JSON.parse(String(row.data)) as Run;
        if (prior.taskId !== taskId || (prior.purpose ?? 'coding') !== 'coding' || prior.taskRevision !== revision || prior.activeLimitMs !== activeLimitMs || JSON.stringify(prior.launch) !== JSON.stringify(launch)) throw new Conflict('Run request identity has different content.');
        return prior;
      }
      if (task.revision !== revision) throw new Conflict('The brief changed. Reload before queuing.');
      if (!task.repositoryId) throw new Conflict('Select a target repository before queuing.');
      if (this.repositoryById(task.repositoryId).retiredAt) throw new Conflict('The selected repository is retired. Select an active repository.');
      if (!task.brief.trim() || !task.criteria.length) throw new Conflict('A brief and acceptance criteria are required.');
      if (task.needsReacceptance) throw new Conflict('Source requirements changed. Review and save the brief before queuing.');
      const existing = this.openRun(taskId);
      if (existing) {
        if ((existing.purpose ?? 'coding') !== 'coding') throw new Conflict('Close the open refinement before queuing coding work.');
        if (existing.taskRevision !== revision || existing.taskSnapshot.sourceRevision !== task.sourceRevision) throw new Conflict('This ticket is already queued with an earlier brief.');
        if (requestId || JSON.stringify(existing.launch) !== JSON.stringify(launch)) throw new Conflict('This ticket already has a different queued run request.');
        return existing;
      }
      if (launch) { parseLaunch(rawLaunch, roles); this.validateLaunchBriefs(task, launch); }
      const run = this.createRun(task, 'coding', activeLimitMs, requestId);
      if (launch) { run.launch = launch; this.saveRun(run); }
      if (requestId) this.recordQueueRequest(requestId, 'queue', payload, run);
      return run;
    });
  }

  importJira(input: unknown): Task[] {
    const batch = jiraImportSchema.parse(input);
    const site = new URL(batch.siteUrl).origin;
    return this.transaction(() => this.importJiraRows(site, batch.issues));
  }

  private importJiraRows(site: string, issues: z.infer<typeof jiraIssueSchema>[]): Task[] {
    return issues.map((issue, rank) => {
      const existing = this.db.prepare('SELECT * FROM tickets WHERE source_site=? AND source_id=?').get(site, issue.id);
      const previous = existing ? this.task(existing) : undefined;
      if (previous?.sourceUpdatedAt && issue.fields.updated && Date.parse(issue.fields.updated) < Date.parse(previous.sourceUpdatedAt)) throw new Conflict('Jira returned an older issue observation.');
      const fields = { key: issue.key, title: issue.fields.summary,
        description: descriptionText(issue.fields.description).trim(), jiraStatus: issue.fields.status.name,
        priority: issue.fields.priority?.name ?? 'Unspecified', project: issue.fields.project?.name ?? '', rank, jiraStatusId: issue.fields.status.id, sourceUpdatedAt: issue.fields.updated };
      const requirementsChanged = previous && (previous.title !== fields.title || previous.description !== fields.description);
      if (requirementsChanged) {
        const run = this.openRun(previous.id);
        if (run) {
          const active = this.runtime(run.id).attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped');
          this.changeRun(run.id, { type: 'sourceChanged', reason: 'Jira requirements changed. Compare and accept the new requirements before continuing this run.', active });
        }
      }
      const source = { ...fields, id: previous?.id ?? randomUUID(), source: 'jira', sourceSite: site,
        sourceId: issue.id, sourceUrl: `${site}/browse/${issue.key}`,
        sourceRevision: (previous?.sourceRevision ?? 0) + (requirementsChanged || !previous ? 1 : 0),
        updatedAt: new Date().toISOString() };
      const refinement = existing ? JSON.parse(String(existing.refinement)) : { revision: 1, brief: '', criteria: [], needsReacceptance: false };
      if (requirementsChanged && refinement.brief) refinement.needsReacceptance = true;
      this.db.prepare(`INSERT INTO tickets VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET imported=excluded.imported, refinement=excluded.refinement`)
        .run(source.id, site, issue.id, JSON.stringify(source), JSON.stringify(refinement));
      return this.getTask(source.id);
    });
  }

  jiraState(): JiraSyncState | undefined {
    const row = this.db.prepare('SELECT data FROM jira_sync WHERE id=1').get(); return row ? JSON.parse(String(row.data)) : undefined;
  }
  commitJiraSync(source: JiraSource, input: unknown, retainedInput: unknown = []) {
    const issues = z.array(jiraIssueSchema).max(1000).refine(items => new Set(items.map(item => item.id)).size === items.length).parse(input);
    const retained = z.array(jiraIssueSchema).max(1000).parse(retainedInput);
    if (new Set([...issues, ...retained].map(issue => issue.id)).size !== issues.length + retained.length) throw new Conflict('Jira synchronization repeated an issue.');
    return this.transaction(() => {
      this.importJiraRows(new URL(source.siteUrl).origin, retained);
      const tasks = this.importJiraRows(new URL(source.siteUrl).origin, issues); const now = new Date().toISOString();
      const state: JiraSyncState = { source, state: 'connected', taskIds: tasks.map(task => task.id), lastSyncedAt: now, lastAttemptAt: now };
      this.db.prepare('INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data').run(JSON.stringify(state)); return state;
    });
  }
  failJiraSync(source: JiraSource, error: string) {
    const saved = this.jiraState();
    const state: JiraSyncState = { ...(saved && JSON.stringify(saved.source) === JSON.stringify(source) ? saved : { taskIds: [] }), source, state: 'unavailable', error, lastAttemptAt: new Date().toISOString() };
    this.db.prepare('INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data').run(JSON.stringify(state));
  }

  /** Accepted revisions are independent of Jira source fields and existing assignment snapshots. */
  acceptRequirements(id: string, input: unknown, provenance: { origin: 'operator' | 'refinement'; proposalId?: string } = { origin: 'operator' }): Task {
    const value = refinementSchema.parse(input);
    if (provenance.origin === 'refinement' && !provenance.proposalId) throw new Conflict('An approved refinement needs its proposal identity.');
    return this.transaction(() => this.saveAcceptedRequirements(id, value, provenance));
  }

  private saveAcceptedRequirements(id: string, value: z.infer<typeof refinementSchema>, provenance: { origin: 'operator' | 'refinement'; proposalId?: string }): Task {
      const previous = this.getTask(id);
      if (previous.revision !== value.revision) throw new Conflict('The brief changed. Reload before saving.');
      if (previous.sourceRevision !== value.sourceRevision) throw new Conflict('Source requirements changed. Reload before saving.');
      const latest = this.requirementRevisions(id).at(-1);
      const revision: RequirementRevision = { taskId: id, revision: previous.revision + 1, brief: value.brief,
        criteria: value.criteria, sourceRevision: value.sourceRevision,
        origin: previous.needsReacceptance || (latest && latest.sourceRevision !== value.sourceRevision) ? 'jira-reacceptance' : provenance.origin,
        ...(provenance.proposalId ? { proposalId: provenance.proposalId } : {}),
        ...(value.decisionNote ? { decisionNote: value.decisionNote } : {}), createdAt: new Date(this.clock()).toISOString() };
      const row = this.db.prepare('SELECT refinement FROM tickets WHERE id=?').get(id)!;
      this.db.prepare('UPDATE tickets SET refinement=? WHERE id=?').run(JSON.stringify({
        ...JSON.parse(String(row.refinement)), brief: revision.brief, criteria: revision.criteria,
        revision: revision.revision, acceptedSourceRevision: value.sourceRevision, needsReacceptance: false,
      }), id);
      this.db.prepare('INSERT INTO requirement_revisions VALUES(?,?,?)').run(id, revision.revision, JSON.stringify(revision));
      return this.getTask(id);
  }

  refine(id: string, input: unknown): Task { return this.acceptRequirements(id, input); }

  /** Explicit, keyed operator decision: an accepted edit alone never rewrites a run snapshot. */
  adoptRequirements(runId: string, input: unknown): Run {
    const value = z.object({ requestId: z.string().uuid(), revision: z.number().int().positive(),
      reason: z.string().trim().min(1).max(4000).optional() }).strict().parse(input);
    return this.transaction(() => {
      const run = this.getRun(runId);
      const prior = run.adoptions?.find(item => item.requestId === value.requestId);
      if (prior) {
        if (prior.toRevision !== value.revision || prior.reason !== value.reason) throw new Conflict('This adoption decision already has different content.');
        return run;
      }
      if (run.desiredState === 'cancelled' && run.status !== 'completed') throw new Conflict('A cancelled run cannot adopt new requirements.');
      if (this.publication(runId)?.merged) throw new Conflict('A merged run cannot adopt new requirements.');
      const task = this.getTask(run.taskId); const revision = this.requirementRevisions(task.id).at(-1);
      if (!revision || revision.revision !== value.revision || task.revision !== value.revision || task.needsReacceptance || revision.sourceRevision !== task.sourceRevision) throw new Conflict('Accept the current source and latest brief before adopting it.');
      if (!task.brief.trim() || !task.criteria.length) throw new Conflict('A non-empty brief and acceptance criteria are required before adoption.');
      if (run.taskRevision >= value.revision) throw new Conflict('The run already uses this revision.');
      const runtime = this.runtime(run.id);
      const claimed = runtime.attempts.some(attempt => attempt.state !== 'admitted');
      const active = claimed || run.status === 'running' || (run.desiredState === 'running' && runtime.assignments.length > 0);
      if (active && !value.reason) throw new Conflict('Active adoption requires an operator reason.');
      const now = new Date(this.clock()).toISOString();
      if (run.status === 'completed') {
        if (this.openRun(run.taskId)) throw new Conflict('Another run is open for this ticket. Resolve it before reopening the research outcome.');
        run.status = 'paused'; run.desiredState = 'paused'; run.controlRevision = (run.controlRevision ?? 0) + 1;
      }
      const fromRevision = run.taskRevision;
      run.adoptions = [...(run.adoptions ?? []), { requestId: value.requestId, fromRevision, toRevision: revision.revision, reason: value.reason, at: now }];
      const changed = run.taskSnapshot.criteria.flatMap((criterion, index) => task.criteria[index] !== criterion ? [index] : []);
      for (const assignment of runtime.assignments.filter(item => !item.coordinator && !item.retiredAt)) {
        const indices = assignment.brief?.criteria.filter(item => task.criteria[item.index] !== item.criterion).map(item => item.index) ?? [];
        assignment.rebriefRequired = indices.length ? { adoptedRevision: revision.revision, criterionIndices: indices } : undefined;
        this.db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment), assignment.id);
      }
      run.taskRevision = revision.revision; run.taskSnapshot = task;
      if (run.purpose === 'research') run.researchCompletion = undefined;
      if (run.sourceConflict && revision.sourceRevision === task.sourceRevision) {
        run.sourceConflict = undefined;
        if (run.pauseReason === 'source' && !active) { run.status = 'awaiting_runner'; run.desiredState = 'running'; run.pauseReason = undefined; run.attentionReason = undefined; }
      }
      if (active && (run.status === 'running' || run.status === 'awaiting_runner')) {
        Object.assign(run, transitionRun(run, { type: 'desire', desiredState: 'paused' }, this.clock()));
        Object.assign(run, transitionRun(run, { type: 'beginPause', reason: 'requirements' }, this.clock()));
      }
      this.saveRun(run);
      for (const proposal of this.teamProposals(runId).filter(item => item.origin === 'operator' && item.status === 'pending')) {
        this.rejectUnavailableTeamProposalInTransaction(proposal, []);
      }
      const work = this.repositoryWork(runId);
      if (work) { work.requirementsHash = requirementsHash(task, revision.revision); this.saveRepositoryWork(work); }
      if (active) {
        const message: Message = { id: randomUUID(), runId, createdAt: now, delivery: 'pending', transitions: [], kind: 'requirements_adoption',
          body: `Operator adopted accepted requirements revision ${revision.revision} from ${fromRevision}. Reason: ${value.reason}. Accepted task: ${JSON.stringify(task)}. Changed criterion indices: ${JSON.stringify(changed)}. Existing assignment briefs remain historical; re-brief flagged assignments via the team proposal/retirement path at a safe point. Earlier coding evidence does not cover this revision.` };
        this.db.prepare('INSERT INTO steering VALUES(?,?,?,?)').run(message.id, runId, `adopt:${value.requestId}`, JSON.stringify(message));
      }
      return run;
    });
  }
}
