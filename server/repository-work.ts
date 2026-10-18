import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { accessSync, closeSync, constants, existsSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, readlinkSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { lstat, readFile, readlink } from 'node:fs/promises';
import { promisify } from 'node:util';
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { z } from 'zod';
import { repositoryRequestSchema } from './repository-request';
import { Conflict, NotFound, Workbench } from './workbench';
import type { CompletionOperation, ImplementationClaim } from '../shared/evidence';
import type { RepositoryCheckpoint, RepositoryRevision, RepositoryStatus, RepositoryWorkRecord, TargetRefresh, TargetRefreshWorkspace, TargetConflictContinuationResult } from '../shared/repository';
import { requirementsHash } from '../shared/requirements';
import { describeCheckedOwned, describeCheckedSpec, inspectCheckedOwned, launchCheckedOwned, prepareCheckedOwned, sealCheckedOwned, type ControllerAuthority } from './runtime/processes';

const hash = (value: string | Buffer) => createHash('sha256').update(value).digest('hex');
const execGit = promisify(execFile);
type FileEntry = { path: string; mode: number; hash: string };
export class TargetChangedRejection extends Conflict {}

/**
 * Backend-owned Git operations. No shell, hooks, textconv or configured content filters.
 * Every Git call is asynchronous: agent tools and operator routes share the event loop with live observation.
 */
export class RepositoryWork {
  constructor(private store: Workbench, private root: string, private promotionAuthority?: ControllerAuthority) {}
  /** Publication supplies a read-only remote observer; local-only runs retain local-ref verification. */
  setTargetObserver(observer: (work: RepositoryWorkRecord) => Promise<string | undefined>) { this.targetObserver = observer; }
  private targetObserver?: (work: RepositoryWorkRecord) => Promise<string | undefined>;
  private continuationObserver?: (sourceRunId: string) => Promise<void>;
  setContinuationObserver(observer: (sourceRunId: string) => Promise<void>) { this.continuationObserver = observer; }

  async verifyCancelledWork(sourceRunId: string, successorId?: string) {
    const source = this.store.cancelledSource(sourceRunId, successorId);
    if (source.publication && !this.continuationObserver) throw new Conflict('Continuation held: PR verification is unavailable.');
    await this.continuationObserver?.(sourceRunId);
    const observed = await this.status(sourceRunId); const expected = source.checkpoint.revision;
    if (observed.revision.head !== expected.head || observed.revision.worktreeHash !== expected.worktreeHash)
      throw new Conflict('Continuation held: the recorded branch head or saved edits changed. Inspect the retained work.');
    if (observed.drift.instructions) throw new Conflict('Continuation held: project instructions changed. Inspect the retained work before a new run.');
    const current = this.store.cancelledSource(sourceRunId, successorId);
    if (JSON.stringify(current.work) !== JSON.stringify(source.work)) throw new Conflict('Continuation held: ownership changed during verification.');
    return { ...current, revision: observed.revision, targetMoved: observed.drift.base };
  }

  private async admitCancelledWork(runId: string) {
    const run = this.store.getRun(runId); const link = run.continuedFrom!;
    const source = await this.verifyCancelledWork(link.runId, runId);
    if (source.work.branch !== link.branch || source.revision.head !== link.expectedHead || source.revision.worktreeHash !== link.worktreeHash
      || source.publication?.id !== link.publicationId || source.publication?.pr.id !== link.prId)
      throw new Conflict('Continuation held: the captured branch or PR identity changed while queued.');
    const dependencyHold = await this.dependencyBaseHold(runId, source.work.base);
    if (dependencyHold) throw new Conflict(`Continuation held: ${dependencyHold}`);
    const current = this.store.getRun(runId);
    return this.store.transferCancelledWork(runId, source.work, source.configured, { id: randomUUID(), at: new Date().toISOString(),
      reason: 'prepared', revision: { ...source.revision, base: source.work.base, requirementsHash: requirementsHash(current.taskSnapshot, current.adoptions?.length ? current.taskRevision : undefined) } },
      source.work.continuationTargetRef ?? (source.targetMoved ? source.work.targetRef : undefined));
  }
  /** Tests supply a real controller after creating the standard repository fixture. Production passes it at construction. */
  setPromotionAuthority(authority: ControllerAuthority) { this.promotionAuthority = authority; }
  private authority() {
    if (!this.promotionAuthority) throw new Conflict('Checked Git promotion requires current controller authority.');
    this.promotionAuthority.assertCurrent();
    return this.promotionAuthority;
  }
  private gitExecutable() {
    for (const directory of (process.env.PATH ?? '').split(':')) {
      if (!isAbsolute(directory)) continue;
      const path = join(directory, 'git');
      try { accessSync(path, constants.X_OK); return realpathSync(path); } catch { /* Try the next executable. */ }
    }
    throw new Conflict('Git executable is unavailable for checked promotion.');
  }
  /** Resolve the immutable command and random one-shot root; no launch directory or manifest is created here. */
  async describePromotionProcess(work: RepositoryWorkRecord, operationId: string, toHead: string): Promise<NonNullable<TargetRefreshWorkspace['promotionProcess']>> {
    const authority = this.authority();
    const parent = join(realpathSync(this.root), 'promotion-processes');
    mkdirSync(parent, { recursive: true, mode: 0o700 });
    const directory = openSync(this.root, 'r'); try { fsyncSync(directory); } finally { closeSync(directory); }
    let filters = '';
    try { filters = await this.git(work.path, ['config', '--includes', '--name-only', '--get-regexp', '^filter\\.']); }
    catch (error) { if (!(error instanceof Conflict && (error.cause as { code?: number } | undefined)?.code === 1)) throw error; }
    const command = this.gitExecutable();
    const args = [...this.gitOverrides(filters), 'reset', '--keep', '--no-recurse-submodules', toHead];
    const env = { PATH: process.env.PATH ?? '', HOME: process.env.HOME ?? '', LANG: 'C', GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
    const token = randomUUID();
    const descriptor = describeCheckedOwned(join(realpathSync(parent), `${operationId}-${token}`), token, command, args, { cwd: work.path, env }, authority);
    return { ...descriptor, command, args, cwd: work.path, env };
  }
  /** Materialize only an exact persisted promoting descriptor, never an orphan speculative launch. */
  preparePromotionProcess(runId: string, requestId: string, operationId: string) {
    const work = this.store.repositoryWork(runId);
    const refresh = work?.refreshes?.find(item => item.requestId === requestId);
    const completion = this.store.codingRecord(runId)?.completions?.find(item => item.id === operationId && item.conflictRefreshId === requestId && item.state === 'requested');
    if (!work || refresh?.state !== 'conflicted' || refresh.resolutionWorkspace?.state !== 'promoting' || !completion)
      throw new Conflict('Persist the exact promotion intent before preparing its checked Git launch.');
    const process = this.promotionProcess(work, refresh, completion, true);
    const authority = this.authority();
    if (process.epoch !== authority.epoch || process.controllerRoot !== authority.root) throw new Conflict('Promotion launch belongs to an older controller epoch.');
    const expected = describeCheckedOwned(process.root, process.token, process.command, process.args, { cwd: process.cwd, env: process.env }, authority);
    if (expected.specSha256 !== process.specSha256) throw new Conflict('Prepared Git manifest would differ from the recorded intent.');
    prepareCheckedOwned(process.root, process.token, process.command, process.args, { cwd: process.cwd, env: process.env }, authority);
  }
  private promotionProcess(work: RepositoryWorkRecord, refresh: TargetRefresh, completion: CompletionOperation, newLaunch = false) {
    const process = refresh.resolutionWorkspace?.promotionProcess;
    const args = process?.args;
    const options = args?.slice(0, -4);
    const safeOptions = options?.length && JSON.stringify(options.slice(0, 6)) === JSON.stringify(['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'submodule.recurse=false'])
      && options.length % 2 === 0 && options.slice(6).every((arg, index) => index % 2 === 0 ? arg === '-c'
        : /^filter\.[^=]+\.(clean|smudge|process)=$|^filter\.[^=]+\.required=false$/.test(arg));
    if (!process || process.version !== 1 || process.root !== join(realpathSync(this.root), 'promotion-processes', `${completion.id}-${process.token}`)
      || process.cwd !== work.path || (newLaunch && process.command !== this.gitExecutable()) || !safeOptions
      || JSON.stringify(args?.slice(-4)) !== JSON.stringify(['reset', '--keep', '--no-recurse-submodules', refresh.resolutionWorkspace?.toHead]))
      throw new Conflict('Checked Git promotion identity is unavailable or changed; retain the recorded intent.');
    const expected = describeCheckedSpec(process.root, process.token, process.command, process.args, { cwd: process.cwd, env: process.env }, process.epoch, process.controllerRoot);
    if (expected.specSha256 !== process.specSha256) throw new Conflict('Checked Git promotion descriptor differs from its admitted command.');
    if (existsSync(join(process.root, 'launch.json'))) {
      const manifest = JSON.parse(readFileSync(join(process.root, 'launch.json'), 'utf8'));
      if (manifest.command !== process.command || manifest.cwd !== process.cwd || JSON.stringify(manifest.args) !== JSON.stringify(process.args)
        || JSON.stringify(manifest.env) !== JSON.stringify(Object.fromEntries(Object.entries(process.env).sort(([a], [b]) => a.localeCompare(b))))
        || manifest.controllerRoot !== process.controllerRoot || manifest.epoch !== process.epoch)
        throw new Conflict('Checked Git promotion manifest does not match its recorded operation.');
    }
    return process;
  }
  private async verifyCurrentTarget(work: RepositoryWorkRecord, refresh: TargetRefresh): Promise<{ state: 'matched' } | { state: 'changed'; observedTarget: string }> {
    if (await this.ref(work.repository.path, refresh.targetRef) !== refresh.toBase) throw new Conflict('The pinned target snapshot changed; original branch is retained.');
    const live = refresh.targetRef.startsWith('refs/crewroom/')
      ? await this.targetObserver?.(work) : await this.ref(work.repository.path, refresh.targetRef);
    if (!live) throw new Conflict('The live target is unavailable; original branch is retained.');
    return live === refresh.toBase ? { state: 'matched' } : { state: 'changed', observedTarget: live };
  }
  private async rejectChangedConflict(runId: string, refresh: TargetRefresh, completion: CompletionOperation, observedTarget: string) {
    await this.terminalUnpromotedConflict(runId, refresh, completion, { code: 'target_changed', observedTarget });
    throw new TargetChangedRejection('The live target changed before promotion; the resolved workspace is retained for an operator retry.');
  }
  private async terminalUnpromotedConflict(runId: string, refresh: TargetRefresh, completion: CompletionOperation,
    outcome: { code: 'target_changed'; observedTarget: string } | { code: 'promotion_interrupted' }) {
    const original = this.store.repositoryWork(runId);
    const workspace = refresh.resolutionWorkspace;
    if (!original || !workspace?.toHead || !workspace.gitFile || completion.state !== 'requested') throw new Conflict('Unpromoted conflict ownership is uncertain.');
    if (workspace.state === 'promoting') {
      this.promotionAuthority?.assertCurrent();
      const ownership = await sealCheckedOwned(this.promotionProcess(original, refresh, completion));
      if (ownership.state !== 'stopped') throw new Conflict('Checked Git reset is live or uncertain; retain the promoting completion for inspection.');
    }
    const { work: isolated } = await this.conflictWork(runId, refresh.requestId);
    if (await this.rebaseInProgress(isolated) || await this.ref(isolated.path, 'HEAD') !== workspace.toHead
      || await this.git(isolated.path, ['status', '--porcelain=v1', '--untracked-files=all'])
      || hash(JSON.stringify(await this.files(isolated))) !== completion.worktreeHash)
      throw new Conflict('Isolated resolution changed before terminalizing promotion.');
    const current = await this.status(runId);
    if (current.revision.head !== refresh.fromHead || current.revision.dirty || current.drift.requirements || current.drift.instructions
      || !await this.validatedConflictBaseDrift(runId, current, refresh.requestId)
      || current.path !== original.path || current.branch !== original.branch || current.targetRef !== original.targetRef)
      throw new Conflict('Original branch or ownership changed before terminalizing promotion.');
    this.promotionAuthority?.assertCurrent();
    const proof = {
      originalHead: current.revision.head, originalBase: current.base, originalPath: current.path, originalBranch: current.branch,
      originalTargetRef: current.targetRef, originalGitFile: original.gitFile!, workspaceGitFile: workspace.gitFile,
      targetRef: refresh.targetRef, workspacePath: isolated.path, workspaceBranch: isolated.branch, toHead: workspace.toHead,
      worktreeHash: completion.worktreeHash, requirementsHash: workspace.requirementsHash ?? original.requirementsHash,
      instructionsHash: workspace.instructionsHash ?? hash(JSON.stringify(original.instructions)), expectedTarget: refresh.toBase,
    };
    if (outcome.code === 'target_changed') this.store.rejectUnpromotedConflict(runId, refresh.requestId, completion.id, { ...proof, observedTarget: outcome.observedTarget });
    else this.store.interruptUnpromotedConflict(runId, refresh.requestId, completion.id, proof);
  }
  /** Git observations only; requirements are derived at return time by `buildStatus`. */
  private observed = new Map<string, { key: string; revision: RepositoryRevision; includesBase: boolean }>();
  private pending = new Map<string, Promise<RepositoryStatus>>();
  private preparing = new Map<string, Promise<unknown>>();
  private gitOverrides(filters: string) {
    const configuration = ['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'submodule.recurse=false'];
    for (const key of new Set(filters.trim().split('\n').filter(Boolean).map(key => key.replace(/\.[^.]+$/, '')))) {
      for (const operation of ['clean', 'smudge', 'process']) configuration.push('-c', `${key}.${operation}=`);
      configuration.push('-c', `${key}.required=false`);
    }
    return configuration;
  }
  /** `beforeDispatch` runs synchronously after the configuration probe, immediately before a mutating command starts. */
  private async git(cwd: string, args: string[], preserveText = false, beforeDispatch?: () => void) {
    const env = { PATH: process.env.PATH, HOME: process.env.HOME, LANG: 'C', GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
    const options = { cwd, env, encoding: 'utf8' as const, timeout: 30000, maxBuffer: 16 * 1024 * 1024 };
    let filters = '';
    try { filters = (await execGit('git', ['config', '--includes', '--name-only', '--get-regexp', '^filter\\.'], options)).stdout; }
    catch (error: unknown) { if (!error || typeof error !== 'object' || !('code' in error) || error.code !== 1) throw new Conflict('Repository configuration is unavailable.'); }
    const configuration = this.gitOverrides(filters); beforeDispatch?.();
    try { const { stdout } = await execGit('git', [...configuration, ...args], options); return preserveText ? stdout : stdout.trimEnd(); }
    catch (error) { throw new Conflict(`Git could not complete ${args[0]}. Retained work needs inspection.`, { cause: error }); }
  }
  /** An execFile rejection proves child exit only for a numeric exit code, never for timeout or signal. */
  private gitReturned(error: unknown) {
    const cause = error instanceof Conflict ? error.cause as { code?: unknown; killed?: boolean; signal?: string } | undefined : undefined;
    return typeof cause?.code === 'number' && !cause.killed && !cause.signal;
  }
  private ref(checkout: string, ref: string) { return this.git(checkout, ['rev-parse', '--verify', '--end-of-options', `${ref}^{commit}`]); }
  private async branches(checkout: string) {
    return (await this.git(checkout, ['worktree', 'list', '--porcelain', '-z'])).split('\0\0').filter(Boolean).map(record => Object.fromEntries(record.split('\0').filter(Boolean).map(line => { const index = line.indexOf(' '); return index < 0 ? [line, true] : [line.slice(0, index), line.slice(index + 1)]; })));
  }
  private async commonDir(checkout: string) { return realpathSync(await this.git(checkout, ['rev-parse', '--path-format=absolute', '--git-common-dir'])); }
  /** Observe the checkout target without fetching or modifying refs; the checkout owner updates stale targets. */
  async dependencyBaseHold(runId: string, preparedBase?: string): Promise<string | undefined> {
    const run = this.store.getRun(runId);
    const dependencies = this.store.dependencyMerges(run);
    if (!dependencies.length) return;
    const repository = this.store.snapshot().repositories.find(item => item.id === run.taskSnapshot.repositoryId);
    if (!repository) return 'Selected repository unavailable';
    let checkout: string;
    try { checkout = realpathSync(repository.path); }
    catch { return `Update target branch ${repository.targetBranch} in the checkout: repository unavailable`; }
    let target = preparedBase;
    if (!target) {
      try { target = await this.ref(checkout, `refs/heads/${repository.targetBranch}`); }
      catch {
        try { target = await this.ref(checkout, `refs/remotes/${repository.targetBranch}`); }
        catch { return `Update target branch ${repository.targetBranch} in the checkout: target unavailable`; }
      }
    }
    for (const { key, commit } of dependencies) {
      try { await this.git(checkout, ['merge-base', '--is-ancestor', commit, target]); }
      catch { return `${key}: verified merge ${commit.slice(0, 12)} is not in prepared base ${repository.targetBranch}; update the target branch in this checkout`; }
    }
  }
  private metadata(work: RepositoryWorkRecord) {
    if (!existsSync(work.path) || realpathSync(work.path) !== work.path) throw new Conflict('Owned worktree path is unavailable or changed.');
    const gitPath = join(work.path, '.git');
    if (!lstatSync(gitPath).isFile() || lstatSync(gitPath).isSymbolicLink()) throw new Conflict('Owned Git metadata changed.');
    const gitFile = readFileSync(gitPath, 'utf8');
    if (work.gitFile && gitFile !== work.gitFile) throw new Conflict('Owned Git metadata changed.');
    // Validate the path before executing Git in the worktree; an edited .git file cannot redirect it.
    const metadata = gitFile.match(/^gitdir: (.+)\n?$/)?.[1];
    const metadataPath = metadata && resolve(work.path, metadata);
    if (!metadataPath || !metadataPath.startsWith(join(work.commonDir, 'worktrees') + '/') || !existsSync(metadataPath) || realpathSync(metadataPath) !== metadataPath) throw new Conflict('Owned Git metadata is outside its reserved repository.');
    return gitFile;
  }
  private async validate(work: RepositoryWorkRecord) {
    const gitFile = this.metadata(work);
    const [branches, directory] = await Promise.all([this.branches(work.repository.path), this.commonDir(work.path)]);
    const listed = branches.find(item => item.worktree === work.path);
    if (!listed || listed.branch !== `refs/heads/${work.branch}` || directory !== work.commonDir) throw new Conflict('Worktree branch or repository ownership changed.');
    return gitFile;
  }
  private async files(work: RepositoryWorkRecord) {
    const names = (await this.git(work.path, ['ls-files', '-z', '--cached', '--others', '--exclude-standard'])).split('\0').filter(Boolean);
    const files: FileEntry[] = [];
    for (const name of [...new Set(names)].sort()) {
      const path = resolve(work.path, name); const rel = relative(work.path, path);
      if (rel.startsWith('../') || isAbsolute(rel) || rel.split('/').includes('.git')) throw new Conflict('Repository path escaped the owned worktree.');
      // Parent symlinks must not turn a tracked child into a host-side read.
      let parent = dirname(path);
      while (parent !== work.path) { if (existsSync(parent) && (await lstat(parent)).isSymbolicLink()) throw new Conflict('A repository parent directory is a symbolic link.'); parent = dirname(parent); }
      let stat;
      try { stat = await lstat(path); }
      catch (error: unknown) { if (error && typeof error === 'object' && 'code' in error && error.code === 'ENOENT') { files.push({ path: name, mode: 0, hash: 'deleted' }); continue; } throw error; }
      if (stat.isSymbolicLink()) files.push({ path: name, mode: stat.mode, hash: hash(await readlink(path)) });
      else {
        if (!stat.isFile() || stat.size > 50 * 1024 * 1024) throw new Conflict('Repository observation requires regular files smaller than 50 MB; submodules need explicit support.');
        files.push({ path: name, mode: stat.mode, hash: hash(await readFile(path)) });
      }
    }
    return files;
  }
  private async instructions(work: RepositoryWorkRecord, files: FileEntry[]) {
    const instructions = [];
    for (const file of files.filter(item => ['AGENTS.md', 'CLAUDE.md'].includes(basename(item.path)) && item.hash !== 'deleted')) {
      const path = join(work.path, file.path);
      if (!(await lstat(path)).isFile()) throw new Conflict('Project instructions must be regular files in the owned worktree.');
      const body = await readFile(path, 'utf8'); if (body.length > 100000) throw new Conflict('Project instructions exceed the admission limit.');
      instructions.push({ path: file.path, body, sha256: file.hash });
    }
    return instructions;
  }
  private async currentInstructions(work: RepositoryWorkRecord) {
    const names = (await this.git(work.path, ['ls-files', '-z', '--cached', '--others', '--exclude-standard'])).split('\0').filter(Boolean);
    const entries: FileEntry[] = [];
    for (const name of [...new Set(names)].filter(name => ['AGENTS.md', 'CLAUDE.md'].includes(basename(name))).sort()) {
      const path = this.scopedPath(work, name);
      try {
        const stat = await lstat(path);
        entries.push({ path: name, mode: stat.mode, hash: stat.isFile() ? hash(await readFile(path)) : 'invalid' });
      } catch (error) {
        if (!error || typeof error !== 'object' || !('code' in error) || error.code !== 'ENOENT') throw error;
        entries.push({ path: name, mode: 0, hash: 'deleted' });
      }
    }
    return this.instructions(work, entries);
  }
  private requirementsHash(work: RepositoryWorkRecord) {
    const run = this.store.getRun(work.runId);
    return requirementsHash(this.store.getTask(run.taskId), run.adoptions?.length ? run.taskRevision : undefined);
  }
  /** Revision of an already validated worktree. */
  private async revision(work: RepositoryWorkRecord): Promise<RepositoryRevision> {
    const [base, head, tree, porcelain, files] = await Promise.all([
      this.ref(work.repository.path, work.targetRef),
      this.ref(work.path, 'HEAD'),
      this.git(work.path, ['rev-parse', 'HEAD^{tree}']),
      this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all', '--ignore-submodules=none']),
      this.files(work),
    ]);
    const instructionsHash = hash(JSON.stringify(await this.instructions(work, files)));
    // Requirements are read after the last await, so a refinement during Git is part of this revision.
    return { base, head, tree, worktreeHash: hash(JSON.stringify(files)), dirty: !!porcelain, requirementsHash: this.requirementsHash(work), instructionsHash };
  }
  /**
   * Callers must build status synchronously after their last await. Requirements are re-read here because a
   * refinement can land during any Git probe, and a stale hash would make superseded evidence look current.
   */
  private buildStatus(work: RepositoryWorkRecord, observed: RepositoryRevision, includesBase: boolean): RepositoryStatus {
    const revision = { ...observed, requirementsHash: this.requirementsHash(work) };
    const prior = work.checkpoints.at(-1)?.revision;
    return { ...work, revision, drift: { base: revision.base !== work.base || !includesBase, head: prior ? revision.head !== prior.head : false,
      worktree: prior ? revision.worktreeHash !== prior.worktreeHash : false, requirements: revision.requirementsHash !== work.requirementsHash,
      instructions: revision.instructionsHash !== hash(JSON.stringify(work.instructions)) } };
  }
  private async checkpoint(work: RepositoryWorkRecord, reason: RepositoryCheckpoint['reason'], attemptId?: string): Promise<RepositoryCheckpoint> {
    await this.validate(work);
    return { id: randomUUID(), at: new Date().toISOString(), reason, attemptId, revision: await this.revision(work) };
  }
  /** Admission is serialized per run: a repeated request waits for, then adopts, the first one's durable state. */
  prepare(runId: string, input: unknown): Promise<RepositoryWorkRecord> {
    const next = (this.preparing.get(runId) ?? Promise.resolve()).catch(() => {}).then(() => this.admit(runId, input));
    this.preparing.set(runId, next);
    return next.finally(() => { if (this.preparing.get(runId) === next) this.preparing.delete(runId); });
  }
  /** Present only actionable, path-free preparation failures to queued operators. */
  preparationHoldReason(error: unknown): string {
    if (error instanceof Conflict && /^(Continuation held:|Selected repository checkout is unavailable|Selected branch is already checked out|Generated branch already exists|Worktree destination already exists)/.test(error.message))
      return error.message;
    return 'Repository preparation could not complete. Inspect the selected checkout and Save launch details to retry.';
  }
  private async admit(runId: string, input: unknown) {
    const purpose = this.store.getRun(runId).purpose ?? 'coding';
    if (purpose !== 'coding') throw new Conflict('Refinement runs cannot prepare repository work.');
    const request = repositoryRequestSchema.parse(input); let work = this.store.repositoryWork(runId);
    const continuation = this.store.getRun(runId).continuedFrom;
    if (continuation) {
      if (request.mode !== 'continue' || request.branch !== continuation.branch || request.expectedHead !== continuation.expectedHead)
        throw new Conflict('Continuation held: launch must retain the captured branch and head.');
      if (!work) return this.admitCancelledWork(runId);
      if (!this.store.getRun(runId).launchCompletedAt && !this.store.runtime(runId).attempts.some(item => item.state !== 'admitted'))
        await this.verifyCancelledWork(continuation.runId, runId);
    }
    if (work) {
      if (work.mode !== request.mode || (request.mode === 'continue' && (work.branch !== request.branch || work.startHead !== request.expectedHead))) throw new Conflict('Run already has a different repository selection.');
      if (work.state === 'ready') { await this.validate(work); return work; }
    } else {
      const run = this.store.getRun(runId);
      const configured = this.store.snapshot().repositories.find(repo => repo.id === run.taskSnapshot.repositoryId);
      if (!configured) throw new NotFound('Selected repository is unavailable.');
      let checkout: string;
      try { checkout = realpathSync(configured.path); }
      catch (error) { throw new Conflict('Selected repository checkout is unavailable. Restore access, then Save launch details to retry.', { cause: error }); }
      if (await this.git(checkout, ['rev-parse', '--show-toplevel']) !== checkout) throw new Conflict('Select the repository root checkout.');
      const repository = { ...configured, path: checkout };
      const commonDir = await this.commonDir(checkout);
      await this.git(checkout, ['check-ref-format', `refs/heads/${repository.targetBranch}`]);
      let targetRef = `refs/heads/${repository.targetBranch}`;
      try { await this.ref(checkout, targetRef); } catch { targetRef = `refs/remotes/${repository.targetBranch}`; }
      const base = await this.ref(checkout, targetRef);
      const dependencyHold = await this.dependencyBaseHold(runId, base);
      if (dependencyHold) throw new Conflict(dependencyHold);
      const branch = request.mode === 'new' ? `${repository.branchPrefix ?? 'codex/crewroom/'}${runId}` : request.branch;
      await this.git(checkout, ['check-ref-format', `refs/heads/${branch}`]);
      if ((await this.branches(checkout)).some(item => item.branch === `refs/heads/${branch}`)) throw new Conflict('Selected branch is already checked out. Inspect and reconcile its owner, then Save launch details to retry.');
      let startHead = base;
      if (request.mode === 'continue') {
        startHead = await this.ref(checkout, `refs/heads/${branch}`);
        if (startHead !== request.expectedHead) throw new Conflict('Selected branch head changed. Refresh before continuing.');
      } else {
        let exists = false; try { await this.ref(checkout, `refs/heads/${branch}`); exists = true; } catch {}
        if (exists) throw new Conflict('Generated branch already exists without owned state. Inspect and reconcile branch ownership, then Save launch details to retry.');
      }
      mkdirSync(this.root, { recursive: true, mode: 0o700 }); const path = join(realpathSync(this.root), runId);
      if (existsSync(path)) throw new Conflict('Worktree destination already exists without owned state. Inspect and reconcile the destination, then Save launch details to retry.');
      work = this.store.reserveRepositoryWork({ runId, repository, commonDir, path, branch, targetRef, mode: request.mode, base, startHead,
        requirementsHash: requirementsHash(run.taskSnapshot, run.adoptions?.length ? run.taskRevision : undefined), state: 'preparing', instructions: [], checkpoints: [] });
    }
    const pin = `refs/crewroom/${runId}/start`;
    let pinned: string | undefined; try { pinned = await this.ref(work.repository.path, pin); } catch {}
    if (pinned && pinned !== work.startHead) throw new Conflict('Owned admission reference changed.');
    if (!pinned) await this.git(work.repository.path, ['update-ref', pin, work.startHead, '0'.repeat(work.startHead.length)]);
    if (!existsSync(work.path)) {
      if ((await this.branches(work.repository.path)).some(item => item.branch === `refs/heads/${work!.branch}`)) throw new Conflict('Branch became checked out during admission.');
      if (work.mode === 'continue' && await this.ref(work.repository.path, `refs/heads/${work.branch}`) !== work.startHead) throw new Conflict('Branch head changed during admission.');
      await this.git(work.repository.path, work.mode === 'new' ? ['worktree', 'add', '-b', work.branch, work.path, work.startHead] : ['worktree', 'add', work.path, work.branch]);
    }
    const gitFile = await this.validate(work);
    if (await this.ref(work.path, 'HEAD') !== work.startHead) throw new Conflict('Incomplete admission has unexpected commits; inspect before adopting.');
    const files = await this.files(work);
    return this.store.confirmRepositoryWork(runId, gitFile, await this.instructions(work, files), await this.checkpoint(work, 'prepared'));
  }
  /** Uncached revision and drift gate for launch, writer ownership, completion and publication. */
  async status(runId: string): Promise<RepositoryStatus> {
    const work = this.store.repositoryWork(runId); if (!work) throw new NotFound('Repository work has not been prepared.');
    await this.validate(work);
    const revision = await this.revision(work);
    let includesBase = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', work.base, revision.head]); } catch { includesBase = false; }
    return this.buildStatus(work, revision, includesBase);
  }
  async observedStatus(runId: string): Promise<RepositoryStatus> {
    const running = this.pending.get(runId);
    if (running) return running;
    const computation = this.computeObservedStatus(runId);
    this.pending.set(runId, computation);
    try { return await computation; } finally { this.pending.delete(runId); }
  }
  private async computeObservedStatus(runId: string): Promise<RepositoryStatus> {
    const work = this.store.repositoryWork(runId); if (!work) throw new NotFound('Repository work has not been prepared.');
    await this.validate(work);
    const [head, base, porcelain, indexPath] = await Promise.all([
      this.ref(work.path, 'HEAD'),
      this.ref(work.repository.path, work.targetRef),
      this.git(work.path, ['status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignore-submodules=none']),
      this.git(work.path, ['rev-parse', '--path-format=absolute', '--git-path', 'index']),
    ]);
    const statSignature = (path: string) => {
      if (!existsSync(path)) return 'missing';
      const stat = lstatSync(path, { bigint: true });
      return `${stat.mode}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`;
    };
    const changedPaths = porcelain.split('\0').filter(line => line.length > 3 && line[2] === ' ').map(line => line.slice(3));
    const key = hash(JSON.stringify({ work, head, base, porcelain, index: statSignature(indexPath),
      changed: changedPaths.map(path => [path, statSignature(join(work.path, path))]) }));
    const saved = this.observed.get(runId);
    if (saved?.key === key) return this.buildStatus(work, saved.revision, saved.includesBase);
    const revision = await this.revision(work);
    let includesBase = true;
    try { await this.git(work.path, ['merge-base', '--is-ancestor', work.base, revision.head]); } catch { includesBase = false; }
    this.observed.set(runId, { key, revision, includesBase });
    return this.buildStatus(work, revision, includesBase);
  }
  verifyExport(destination: string, input: unknown) {
    const files = z.array(z.object({ path: z.string(), mode: z.number().int(), hash: z.string() })).parse(input);
    for (const file of files) {
      if (!file.path || isAbsolute(file.path) || file.path.split('/').some(part => !part || part === '.' || part === '..' || part.toLowerCase() === '.git')) throw new Conflict('Invalid snapshot manifest path.');
      const path = join(destination, file.path);
      let parent = dirname(path);
      while (parent !== destination) { if (lstatSync(parent).isSymbolicLink()) throw new Conflict('Snapshot parent became a symbolic link.'); parent = dirname(parent); }
      const stat = lstatSync(path);
      if ((!stat.isFile() && !stat.isSymbolicLink()) || stat.size > 50 * 1024 * 1024 || (stat.mode & 0o170111) !== (file.mode & 0o170111)) return false;
      if (hash(stat.isSymbolicLink() ? readlinkSync(path) : readFileSync(path)) !== file.hash) return false;
    }
    return true;
  }

  async exportRevision(runId: string, revision: RepositoryRevision, destination: string) {
    const work = await this.status(runId);
    if (work.writer || work.revision.dirty || JSON.stringify(work.revision) !== JSON.stringify(revision)) throw new Conflict('The candidate revision changed before its check snapshot.');
    if (!isAbsolute(destination) || existsSync(destination)) throw new Conflict('Check snapshot needs a new private destination.');
    mkdirSync(destination, { recursive: true, mode: 0o700 });
    await this.git(work.path, ['checkout-index', '--all', '--ignore-skip-worktree-bits', `--prefix=${destination}/`]);
    for (const file of await this.files(work)) {
      const path = join(destination, file.path); const stat = await lstat(path);
      const content = stat.isSymbolicLink() ? await readlink(path) : await readFile(path);
      if (hash(content) !== file.hash || (stat.mode & 0o111) !== (file.mode & 0o111)) throw new Conflict('Check snapshot differs from the admitted source revision.');
    }
    if (JSON.stringify((await this.status(runId)).revision) !== JSON.stringify(revision)) throw new Conflict('The candidate revision changed during snapshot creation.');
    return realpathSync(destination);
  }

  async commit(runId: string, completion: CompletionOperation) {
    const work = await this.status(runId);
    if (this.store.repositoryWork(runId)?.refreshes?.some(item => item.state === 'rebasing'
      || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
      throw new Conflict('An unverified Git mutation blocks repository completion.');
    if (completion.conflictRefreshId) return this.commitConflict(runId, completion);
    // Pause or cancel records intent immediately and queues its stop behind this transition; each commit mutation honors it.
    const committable = () => {
      const run = this.store.getRun(runId);
      if (this.store.repositoryWork(runId)?.writer || run.status !== 'paused' || run.desiredState !== 'running' || this.store.runtime(runId).attempts.some(attempt => attempt.state !== 'admitted' && attempt.ownership !== 'stopped')) throw new Conflict('Pause and confirm stopped ownership before committing.');
    };
    if (work.writer) throw new Conflict('Pause and confirm stopped ownership before committing.');
    committable();
    if (work.drift.base || work.drift.requirements || work.drift.instructions || work.revision.worktreeHash !== completion.worktreeHash) throw new Conflict('Repository content or requirements changed after completion was requested.');
    const marker = `Crewroom-operation: ${completion.id}`;
    if (work.revision.head !== completion.head) {
      if ((await this.git(work.path, ['log', '-1', '--format=%B'])).split('\n').filter(line => line === marker).length !== 1 ||
        await this.ref(work.path, 'HEAD^') !== completion.head || work.revision.dirty) throw new Conflict('Unexpected commit appeared during completion.');
    } else {
      await this.git(work.path, ['add', '--all', '--', '.'], false, committable);
      const tree = await this.git(work.path, ['write-tree'], false, committable);
      if (tree !== work.revision.tree) await this.git(work.path, ['-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', 'commit', '--no-gpg-sign', '-m', `${completion.summary}\n\n${marker}`], false, committable);
    }
    const checkpoint = await this.checkpoint(work, 'completed', completion.attemptId);
    if (checkpoint.revision.dirty || checkpoint.revision.worktreeHash !== completion.worktreeHash) throw new Conflict('Committed revision does not match the completed files.');
    this.store.checkpointRepository(runId, checkpoint);
    return checkpoint.revision;
  }

  /** Finish a stopped writer's isolated rebase before touching the original checked-out branch. */
  private async commitConflict(runId: string, completion: CompletionOperation) {
    const original = this.store.repositoryWork(runId);
    const refresh = original?.refreshes?.find(item => item.requestId === completion.conflictRefreshId);
    if (completion.state !== 'requested') throw new Conflict('Conflict completion is terminal and cannot be promoted.');
    if (!original || !refresh) throw new Conflict('The conflict completion lost its target refresh.');
    const stopped = () => {
      const run = this.store.getRun(runId);
      if (this.store.repositoryWork(runId)?.writer || run.status !== 'paused' || run.desiredState !== 'running'
        || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Confirm stopped ownership before resolving a target conflict.');
    };
    stopped();
    if (refresh.state === 'completed' && refresh.resolutionWorkspace?.operationId === completion.id) {
      await this.cleanupPromotedConflict(original, refresh); return (await this.status(runId)).revision;
    }
    const workspace = refresh.resolutionWorkspace;
    const promoting = workspace?.state === 'promoting';
    if (!workspace?.gitFile || (!promoting && workspace.state !== 'ready') || (promoting && workspace.operationId !== completion.id)) throw new Conflict('Conflict rebase must be ready for its recorded completion.');
    if (promoting) {
      await this.reconcilePromotion(original, refresh, completion);
      return (await this.status(runId)).revision;
    }
    const { work } = await this.conflictWork(runId, promoting ? refresh.requestId : undefined);
    const before = await this.status(runId);
    if (before.revision.head !== completion.head && !(promoting && before.revision.head === workspace.toHead)) throw new Conflict('Original branch changed during conflict resolution.');
    if (before.revision.dirty || before.drift.requirements || before.drift.instructions
      || (before.drift.base && !await this.validatedConflictBaseDrift(runId, before, promoting ? refresh.requestId : undefined))
      || await this.ref(original.repository.path, refresh.targetRef) !== refresh.toBase) throw new Conflict('The original branch, accepted requirements or target changed; stop for the operator.');
    const toHead = workspace.toHead;
    if (!toHead || await this.rebaseInProgress(work) || await this.ref(work.path, 'HEAD') !== toHead
      || await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all'])
      || hash(JSON.stringify(await this.files(work))) !== completion.worktreeHash) throw new Conflict('Conflict resolution changed after ready state; stop for the operator.');
    const changed = async (from: string, to: string) => (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean).sort().join('\0');
    let containsTarget = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', refresh.toBase, toHead]); } catch { containsTarget = false; }
    const instructionsHash = hash(JSON.stringify(await this.currentInstructions(work)));
    if (!containsTarget || await changed(refresh.fromBase, refresh.fromHead) !== await changed(refresh.toBase, toHead)
      || instructionsHash !== (workspace.instructionsHash ?? hash(JSON.stringify(original.instructions)))) throw new Conflict('The isolated resolution changed source scope or project instructions.');
    const observed = await this.verifyCurrentTarget(original, refresh);
    if (observed.state === 'changed') await this.rejectChangedConflict(runId, refresh, completion, observed.observedTarget);
    if (toHead === refresh.fromHead) throw new Conflict('Conflict promotion has no source-changing Git reset to admit.');
    const process = await this.describePromotionProcess(original, completion.id, toHead);
    this.store.beginTargetPromotion(runId, refresh.requestId, completion.id, toHead, process);
    const admitted = this.store.repositoryWork(runId)?.refreshes?.find(item => item.requestId === refresh.requestId);
    if (!admitted?.resolutionWorkspace?.promotionProcess) throw new Conflict('Promotion descriptor was not saved.');
    this.preparePromotionProcess(runId, refresh.requestId, completion.id);
    await this.promoteConflict(original, admitted, toHead, completion, stopped);
    return (await this.status(runId)).revision;
  }
  private async rebaseInProgress(work: RepositoryWorkRecord) {
    for (const name of ['rebase-merge', 'rebase-apply']) if (existsSync(await this.git(work.path, ['rev-parse', '--path-format=absolute', '--git-path', name]))) return true;
    return false;
  }
  private async promoteConflict(work: RepositoryWorkRecord, refresh: TargetRefresh, toHead: string, completion: CompletionOperation, stopped: () => void) {
    stopped();
    const live = await this.verifyCurrentTarget(work, refresh);
    if (live.state === 'changed') await this.rejectChangedConflict(work.runId, refresh, completion, live.observedTarget);
    if (this.store.getRun(work.runId).status !== 'paused' || this.store.getRun(work.runId).desiredState !== 'running')
      throw new Conflict('Confirm paused ownership and active completion intent before promoting the retained conflict.');
    stopped();
    const current = await this.status(work.runId);
    if (current.revision.head !== refresh.fromHead || current.revision.dirty || current.drift.requirements || current.drift.instructions
      || !await this.validatedConflictBaseDrift(work.runId, current, refresh.requestId)
      || this.store.repositoryWork(work.runId)?.targetRef !== work.targetRef) throw new Conflict('Original branch or target changed during promotion; no files were discarded.');
    const finalTarget = await this.verifyCurrentTarget(work, refresh);
    if (finalTarget.state === 'changed') await this.rejectChangedConflict(work.runId, refresh, completion, finalTarget.observedTarget);
    // The remote observer awaited above can span an external Git change; never use the earlier source proof for dispatch.
    const afterTarget = await this.status(work.runId);
    if (afterTarget.revision.head !== refresh.fromHead || afterTarget.revision.dirty || afterTarget.drift.requirements || afterTarget.drift.instructions
      || !await this.validatedConflictBaseDrift(work.runId, afterTarget, refresh.requestId)
      || this.store.repositoryWork(work.runId)?.targetRef !== work.targetRef)
      throw new Conflict('Original branch or target changed during promotion; no files were discarded.');
    if (this.store.codingRecord(work.runId)?.completions?.find(item => item.id === completion.id)?.state !== 'requested') throw new Conflict('Conflict completion changed before branch promotion.');
    const process = this.promotionProcess(work, refresh, completion, true);
    stopped(); this.authority();
    const child = launchCheckedOwned(process, this.authority());
    child.stderr?.resume(); // Launcher diagnostics are separate from the bounded Git stdout/stderr files.
    const result = await new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once('error', reject); child.once('close', (code, signal) => resolve({ code, signal }));
    });
    if (result.code !== 0 || result.signal) throw new Conflict('Checked Git reset failed; retain the promotion intent and inspect its bounded logs.');
    if ((await inspectCheckedOwned(process)).state !== 'stopped') throw new Conflict('Checked Git reset ownership is not stopped.');
    await this.finishPromotion(work, refresh, toHead, completion);
  }
  private async finishPromotion(work: RepositoryWorkRecord, refresh: TargetRefresh, toHead: string, completion: CompletionOperation) {
    const { work: isolated } = await this.conflictWork(work.runId, refresh.requestId);
    if (await this.rebaseInProgress(isolated) || await this.ref(isolated.path, 'HEAD') !== toHead
      || await this.git(isolated.path, ['status', '--porcelain=v1', '--untracked-files=all'])
      || hash(JSON.stringify(await this.files(isolated))) !== completion.worktreeHash)
      throw new Conflict('Resolved workspace changed before stopped promotion was checkpointed.');
    const changed = async (from: string, to: string) => (await this.git(isolated.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean).sort().join('\0');
    try { await this.git(isolated.path, ['merge-base', '--is-ancestor', refresh.toBase, toHead]); }
    catch { throw new Conflict('Stopped promotion no longer contains its pinned target.'); }
    if (await changed(refresh.fromBase, refresh.fromHead) !== await changed(refresh.toBase, toHead)) throw new Conflict('Stopped promotion changed the source-file scope.');
    const current = await this.status(work.runId);
    if (current.revision.head !== toHead || current.revision.dirty || current.revision.worktreeHash !== completion.worktreeHash
      || current.drift.requirements || current.drift.instructions || !await this.validatedConflictBaseDrift(work.runId, current, refresh.requestId))
      throw new Conflict('Promoted branch does not match the stopped checked reset.');
    const moved = { ...work, base: refresh.toBase, targetRef: refresh.targetRef };
    const checkpoint = await this.checkpoint(moved, 'refreshed', completion.attemptId);
    if (checkpoint.revision.head !== toHead || checkpoint.revision.base !== refresh.toBase || checkpoint.revision.dirty) throw new Conflict('Promoted branch did not match the isolated resolved revision.');
    const currentRefresh = this.store.repositoryWork(work.runId)?.refreshes?.find(item => item.requestId === refresh.requestId);
    const workspace = currentRefresh?.resolutionWorkspace;
    if (currentRefresh?.state !== 'conflicted' || workspace?.state !== 'promoting' || workspace.toHead !== toHead) throw new Conflict('Conflict promotion intent changed.');
    this.promotionAuthority?.assertCurrent();
    const saved = this.store.finishTargetRefresh(work.runId, refresh.requestId, { state: 'completed', toHead, resolutionWorkspace: { ...workspace, state: 'promoted' } }, checkpoint);
    this.observed.delete(work.runId);
    try { await this.cleanupPromotedConflict(work, saved.refreshes!.find(item => item.requestId === refresh.requestId)!); }
    catch { console.error(`Promoted target conflict workspace for run ${work.runId} was retained for inspection.`); }
    try {
      const live = await this.verifyCurrentTarget(work, refresh);
      if (live.state === 'changed') this.store.requireAttention(work.runId, 'The live target moved after the checked reset. Local promotion is retained; refresh and revalidate before publication.');
    } catch { this.store.requireAttention(work.runId, 'The live target could not be verified after the checked reset. Local promotion is retained; verify it before publication.'); }
  }
  /** A recovered intent never relaunches its old-epoch process. No exit receipt alone proves success. */
  private async reconcilePromotion(work: RepositoryWorkRecord, refresh: TargetRefresh, completion: CompletionOperation) {
    this.promotionAuthority?.assertCurrent();
    const process = this.promotionProcess(work, refresh, completion);
    const inspected = await inspectCheckedOwned(process);
    const ownership = inspected.state === 'uncertain' && !inspected.owner ? await sealCheckedOwned(process) : inspected;
    if (ownership.state !== 'stopped') throw new Conflict('Checked Git reset ownership is live or uncertain; retain the promoting completion.');
    const current = await this.status(work.runId);
    if (current.revision.head === refresh.resolutionWorkspace?.toHead && ownership.owner) {
      await this.finishPromotion(work, refresh, refresh.resolutionWorkspace.toHead, completion);
      return;
    }
    if (current.revision.head !== refresh.fromHead || current.revision.dirty) throw new Conflict('Original branch changed or partially reset; retain the promoting completion.');
    const live = await this.verifyCurrentTarget(work, refresh);
    if (live.state === 'changed') await this.rejectChangedConflict(work.runId, refresh, completion, live.observedTarget);
    await this.terminalUnpromotedConflict(work.runId, refresh, completion, { code: 'promotion_interrupted' });
  }

  private async cleanupPromotedConflict(work: RepositoryWorkRecord, refresh: TargetRefresh) {
    const workspace = refresh.resolutionWorkspace;
    if (work.refreshes?.some(item => item.state === 'rebasing'
      || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
      throw new Conflict('An unverified Git mutation blocks workspace cleanup.');
    if (refresh.state !== 'completed' || workspace?.state !== 'promoted') return;
    const expectedPath = join(realpathSync(dirname(work.path)), `${basename(work.path)}-refresh-${refresh.requestId}`);
    const expectedBranch = `${work.branch}-refresh-${refresh.requestId}`;
    if (workspace.path !== expectedPath || workspace.branch !== expectedBranch || !workspace.toHead) throw new Conflict('Promoted conflict workspace identity changed.');
    const listed = (await this.branches(work.repository.path)).find(item => item.worktree === workspace.path);
    if (existsSync(workspace.path)) {
      const isolated = { ...work, path: workspace.path, branch: workspace.branch, base: refresh.toBase, startHead: refresh.fromHead, targetRef: refresh.targetRef, gitFile: workspace.gitFile };
      if (!listed || listed.branch !== `refs/heads/${workspace.branch}`) throw new Conflict('Promoted worktree ownership changed.');
      await this.validate(isolated);
      if (await this.ref(workspace.path, 'HEAD') !== workspace.toHead || await this.rebaseInProgress(isolated)
        || await this.git(workspace.path, ['status', '--porcelain=v1', '--untracked-files=all'])) throw new Conflict('Promoted conflict worktree is not clean.');
      await this.git(work.repository.path, ['worktree', 'remove', workspace.path]);
    } else if (listed) throw new Conflict('Promoted worktree listing has no matching directory.');
    const branchHead = await this.git(work.repository.path, ['for-each-ref', '--format=%(objectname)', `refs/heads/${workspace.branch}`]);
    if (branchHead && branchHead !== workspace.toHead) throw new Conflict('Promoted temporary branch moved.');
    if (branchHead) await this.git(work.repository.path, ['update-ref', '-d', `refs/heads/${workspace.branch}`, workspace.toHead]);
  }

  /** Research reads Git objects from the configured checkout; no worktree, checkout or ref mutation. */
  async pinResearch(repository: { id: string; path: string; targetBranch: string; revision?: number }) {
    const checkout = realpathSync(repository.path);
    if (await this.git(checkout, ['rev-parse', '--show-toplevel']) !== checkout) throw new Conflict('Select the repository root checkout.');
    await this.git(checkout, ['check-ref-format', `refs/heads/${repository.targetBranch}`]);
    let commit: string;
    try { commit = await this.ref(checkout, `refs/heads/${repository.targetBranch}`); }
    catch { commit = await this.ref(checkout, `refs/remotes/${repository.targetBranch}`); }
    return { repositoryId: repository.id, repositoryRevision: repository.revision ?? 1, path: checkout, commit };
  }

  async readResearch(runId: string, name: string) {
    this.validateName(name);
    const source = this.store.getRun(runId).researchSource;
    if (!source) throw new Conflict('This research run has no pinned repository.');
    const configured = this.store.snapshot().repositories.find(item => item.id === source.repositoryId);
    if (!configured || realpathSync(configured.path) !== source.path) throw new Conflict('The pinned repository checkout is unavailable.');
    const object = `${source.commit}:${name}`;
    let type: string; let size: string;
    try { [type, size] = await Promise.all([this.git(source.path, ['cat-file', '-t', object]), this.git(source.path, ['cat-file', '-s', object])]); }
    catch { throw new NotFound(`Pinned source path ${name} does not exist at ${source.commit}.`); }
    if (type !== 'blob' || Number(size) > 1024 * 1024) throw new Conflict('Pinned source must be a text blob smaller than 1 MB.');
    const content = await this.git(source.path, ['cat-file', 'blob', object], true);
    return { path: name, content, sha256: hash(content), commit: source.commit };
  }

  async searchResearch(runId: string, input: unknown) {
    const value = z.object({ query: z.string().min(1).max(500), regex: z.boolean().default(false), ignoreCase: z.boolean().default(false), path: z.string().max(1000).optional() }).strict().parse(input);
    const source = this.store.getRun(runId).researchSource;
    if (!source) throw new Conflict('This research run has no pinned repository.');
    const configured = this.store.snapshot().repositories.find(item => item.id === source.repositoryId);
    if (!configured || realpathSync(configured.path) !== source.path) throw new Conflict('The pinned repository checkout is unavailable.');
    if (value.path) { if (value.path.startsWith(':')) throw new Conflict('Search paths are plain repository paths.'); this.validateName(value.path.replace(/\/$/, '')); }
    const args = ['grep', '-z', '-n', '--no-column', '--full-name', '-I', '--no-color', '--max-count=101', ...(value.regex ? ['-E'] : ['-F']), ...(value.ignoreCase ? ['-i'] : []), '-e', value.query, source.commit, '--', ...(value.path ? [`:(literal)${value.path}`] : [])];
    let output = '';
    try { output = await this.git(source.path, args, true); }
    catch (error) { const cause = error instanceof Conflict ? error.cause : undefined; if (!cause || typeof cause !== 'object' || !('code' in cause) || cause.code !== 1) throw new Conflict('Pinned search failed. Narrow the query or path.', { cause: error }); }
    const lines = output.split('\n').filter(Boolean);
    return { query: value.query, path: value.path, revision: source.commit,
      matches: lines.slice(0, 100).map(line => { const [file, number, ...rest] = line.split('\0'); return { path: file.slice(source.commit.length + 1), line: Number(number), text: rest.join('\0').slice(0, 300) }; }), truncated: lines.length > 100 };
  }

  private async pinnedTree(runId: string, revision: RepositoryRevision) {
    const work = this.store.repositoryWork(runId); if (!work) throw new NotFound('Repository work is unavailable.');
    await this.validate(work);
    z.string().regex(/^[a-f0-9]{40,64}$/).parse(revision.head);
    if (await this.git(work.path, ['rev-parse', `${revision.head}^{tree}`]) !== revision.tree) throw new Conflict('Candidate tree identity changed.');
    return work;
  }
  async diffRevision(runId: string, revision: RepositoryRevision) {
    z.string().regex(/^[a-f0-9]{40,64}$/).parse(revision.base);
    const work = await this.pinnedTree(runId, revision);
    const patch = await this.git(work.path, ['diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--no-color', revision.base, revision.head, '--'], true);
    if (Buffer.byteLength(patch) > 1024 * 1024) throw new Conflict('The patch exceeds the 1 MB preview limit. Inspect the retained branch locally.');
    return { base: revision.base, head: revision.head, tree: revision.tree, patch };
  }

  async readRevision(runId: string, revision: RepositoryRevision, name: string) {
    this.validateName(name);
    const work = await this.pinnedTree(runId, revision);
    const object = `${revision.head}:${name}`;
    const [type, size] = await Promise.all([this.git(work.path, ['cat-file', '-t', object]), this.git(work.path, ['cat-file', '-s', object])]);
    if (type !== 'blob' || Number(size) > 1024 * 1024) throw new Conflict('Read one bounded text file from the candidate.');
    const content = await this.git(work.path, ['cat-file', 'blob', object], true);
    return { path: name, content, sha256: hash(content), head: revision.head, tree: revision.tree };
  }
  private validateName(name: string) {
    if (!name || isAbsolute(name) || name.includes('\\') || name.split('/').some(part => !part || part === '.' || part === '..' || part.toLowerCase() === '.git') || name.includes('\0')) throw new Conflict('File path is outside repository scope.');
  }

  private scopedPath(work: RepositoryWorkRecord, name: string) {
    this.validateName(name);
    const path = join(work.path, name);
    let current = path;
    while (current !== work.path) {
      try { const stat = lstatSync(current); if (stat.isSymbolicLink() || (current === path && (!stat.isFile() || stat.nlink > 1))) throw new Conflict('Repository file path must not follow links or special files.'); }
      catch (error: any) { if (error.code !== 'ENOENT') throw error; }
      current = dirname(current);
    }
    return path;
  }
  /** An unknown Git child takes precedence over later supersession and never reopens an original writer. */
  unresolvedConflict(runId: string) {
    const refreshes = this.store.repositoryWork(runId)?.refreshes;
    return refreshes?.find(item => item.state === 'rebasing' || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? ''))
      ?? refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
  }

  /** Only an in-scope, retained conflict may be edited; ambiguous conflicts stay operator-owned. */
  activeConflict(runId: string) {
    if (this.store.repositoryWork(runId)?.refreshes?.some(item => item.state === 'rebasing'
      || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? ''))) return undefined;
    const refresh = this.store.repositoryWork(runId)?.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
    return ['conflicted', 'ready'].includes(refresh?.resolutionWorkspace?.state ?? '') ? refresh : undefined;
  }
  private async conflictWork(runId: string, requestId?: string, allowContinuing = false) {
    const original = this.store.repositoryWork(runId);
    const latest = original?.refreshes?.findLast(item => item.state === 'conflicted' && !item.supersededBy);
    const refresh = requestId ? latest?.requestId === requestId ? latest : undefined : this.activeConflict(runId);
    const workspace = refresh?.resolutionWorkspace;
    if (!original || !refresh || !['conflicted', 'ready', 'promoting', ...(allowContinuing ? ['continuing'] : [])].includes(workspace?.state ?? '') || (!requestId && workspace?.state === 'promoting')
      || !workspace?.gitFile || workspace.path !== join(realpathSync(dirname(original.path)), `${basename(original.path)}-refresh-${refresh.requestId}`)
      || workspace.branch !== `${original.branch}-refresh-${refresh.requestId}`) throw new Conflict('Conflict resolution workspace is not available.');
    const source = this.store.codingRecord(runId)?.candidates.at(-1);
    if (!refresh.sourceCandidateId || source?.id !== refresh.sourceCandidateId || source.revision.base !== refresh.fromBase
      || source.revision.head !== refresh.fromHead || refresh.fromBase !== original.base)
      throw new Conflict('Conflict source candidate or accepted base changed; stop for the operator.');
    try { await this.git(original.repository.path, ['merge-base', '--is-ancestor', refresh.fromBase, refresh.fromHead]); }
    catch { throw new Conflict('Conflict source no longer contains its accepted base.'); }
    const work = { ...original, path: workspace.path, branch: workspace.branch, gitFile: workspace.gitFile };
    this.metadata(work);
    const worktrees = await this.branches(original.repository.path);
    const isolatedWorktree = worktrees.find(item => item.worktree === work.path);
    const originalWorktree = worktrees.find(item => item.worktree === original.path);
    const attachedBranch = `refs/heads/${workspace.branch}`;
    const rebasing = await this.rebaseInProgress(work);
    const isolatedBranchValid = ['conflicted', 'continuing'].includes(workspace.state) && rebasing
      ? isolatedWorktree?.branch === attachedBranch || isolatedWorktree?.detached === true
      : isolatedWorktree?.branch === attachedBranch;
    const branchHead = await this.ref(original.repository.path, attachedBranch);
    const workspaceHead = await this.ref(work.path, 'HEAD');
    const originalHead = await this.ref(original.path, 'HEAD');
    if (!isolatedBranchValid || originalWorktree?.branch !== `refs/heads/${original.branch}` || await this.commonDir(work.path) !== original.commonDir
      || (originalHead !== refresh.fromHead && !(workspace.state === 'promoting' && originalHead === workspace.toHead))
      || (branchHead !== refresh.fromHead && branchHead !== workspaceHead)
      || await this.ref(original.repository.path, refresh.targetRef) !== refresh.toBase
      || await this.git(original.path, ['status', '--porcelain=v1', '--untracked-files=all', '--ignore-submodules=none'])
      || (workspace.state === 'promoting' && (!workspace.operationId || !workspace.toHead
        || !this.store.codingRecord(runId)?.completions?.some(item => item.id === workspace.operationId && item.conflictRefreshId === refresh.requestId)))) throw new Conflict('Conflict workspace or target changed; stop for the operator.');
    if (this.requirementsHash(original) !== (workspace.requirementsHash ?? original.requirementsHash)) throw new Conflict('Accepted requirements changed; stop for the operator.');
    const instructionsHash = hash(JSON.stringify(await this.currentInstructions(work)));
    if (instructionsHash !== (workspace.instructionsHash ?? hash(JSON.stringify(original.instructions)))) throw new Conflict('Project instructions changed; stop for the operator.');
    return { work, refresh };
  }
  /** Raw status keeps the fetched target drift; only the exact retained conflict can authorize this difference. */
  async validatedConflictBaseDrift(runId: string, current: RepositoryStatus, requestId?: string) {
    const active = requestId ? this.unresolvedConflict(runId)?.requestId === requestId : !!this.activeConflict(runId);
    if (!active) return !requestId && !current.drift.base;
    const { refresh } = await this.conflictWork(runId, requestId);
    const original = this.store.repositoryWork(runId);
    if (!original || original.base !== current.base || original.targetRef !== current.targetRef
      || (current.revision.head !== refresh.fromHead && !(requestId && current.revision.head === refresh.resolutionWorkspace?.toHead)))
      return false;
    if (!current.drift.base) return true;
    return original.targetRef === refresh.targetRef && current.revision.base === refresh.toBase;
  }
  private async conflictPathsWithinScope(work: RepositoryWorkRecord, refresh: TargetRefresh, paths: string[]) {
    if (!paths.length || paths.some(name => ['AGENTS.md', 'CLAUDE.md'].includes(basename(name)))) return false;
    const changed = async (from: string, to: string) => new Set((await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean));
    const [sourcePaths, targetPaths] = await Promise.all([changed(refresh.fromBase, refresh.fromHead), changed(refresh.fromBase, refresh.toBase)]);
    return paths.every(name => sourcePaths.has(name) && targetPaths.has(name));
  }


  async validateConflictWorkspace(runId: string) {
    const refresh = this.activeConflict(runId);
    if (!refresh || !refresh.resolutionWorkspace) throw new Conflict('No safe isolated target conflict is available.');
    const workspace = refresh.resolutionWorkspace;
    let work: RepositoryWorkRecord;
    try { work = (await this.conflictWork(runId)).work; }
    catch (error) {
      this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, state: 'operator_required' });
      throw error;
    }
    const rebasing = await this.rebaseInProgress(work);
    if (workspace.state === 'conflicted') {
      if (rebasing) {
        const conflicts = (await this.git(work.path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort();
        if (!await this.conflictPathsWithinScope(work, refresh, conflicts)) {
          this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, state: 'operator_required' }, conflicts);
          throw new Conflict('Conflict paths or project instructions exceed the accepted scope; operator decision required.');
        }
        this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, state: 'conflicted' }, conflicts);
        return { requestId: refresh.requestId, state: 'conflicted' as const, conflicts };
      }
      const run = this.store.getRun(runId);
      if (run.status !== 'awaiting_runner' || run.desiredState !== 'running' || work.writer
        || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Explicit Resume and stopped ownership are required to recover the isolated rebase.');
      const head = await this.ref(work.path, 'HEAD');
      const sourceNames = (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', refresh.fromBase, refresh.fromHead, '--'])).split('\0').filter(Boolean).sort().join('\0');
      const rebasedNames = (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', refresh.toBase, head, '--'])).split('\0').filter(Boolean).sort().join('\0');
      let containsTarget = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', refresh.toBase, head]); } catch { containsTarget = false; }
      const clean = !(await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all']));
      if (!containsTarget || !clean || sourceNames !== rebasedNames) {
        this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, state: 'operator_required', toHead: head }, []);
        throw new Conflict('The interrupted rebase changed source scope; operator decision required.');
      }
      this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, state: 'ready', toHead: head }, refresh.conflicts ?? []);
      return { requestId: refresh.requestId, state: 'ready' as const, conflicts: refresh.conflicts ?? [] };
    }
    if (workspace.state === 'ready') {
      if (rebasing || !workspace.toHead || await this.ref(work.path, 'HEAD') !== workspace.toHead
        || await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all'])) throw new Conflict('Recorded ready state no longer matches the isolated Git workspace.');
    }
    return { requestId: refresh.requestId, state: workspace.state, conflicts: refresh.conflicts ?? [] };
  }
  async readConflict(runId: string, name: string) { return this.readIsolatedFile(runId, name, true); }
  private async readIsolatedFile(runId: string, name: string, requireConflict: boolean) {
    const { work, refresh } = await this.conflictWork(runId);
    if (requireConflict && !refresh.conflicts?.includes(name)) throw new Conflict('Only recorded conflict files are in the resolution scope.');
    const path = this.scopedPath(work, name);
    if (!(await this.files(work)).some(file => file.path === name && file.hash !== 'deleted')) throw new NotFound('Isolated repository file is absent.');
    const stat = await lstat(path); if (stat.size > 1024 * 1024) throw new Conflict('Conflict file exceeds the text tool limit.');
    const content = await readFile(path, 'utf8'); return { path: name, content, sha256: hash(content) };
  }
  async conflictFilesHash(runId: string) {
    const { work } = await this.conflictWork(runId);
    return hash(JSON.stringify(await this.files(work)));
  }
  async continueConflict(attemptId: string, requestId: string, toolCallId: string): Promise<TargetConflictContinuationResult> {
    const owner = this.currentWriter(attemptId);
    const held = this.unresolvedConflict(owner.runId);
    const priorWorkspace = held?.requestId === requestId ? held.resolutionWorkspace : undefined;
    const receipt = priorWorkspace?.continuationReceipts?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (receipt) {
      if (receipt.requestId !== requestId || priorWorkspace?.continuationReceipts?.at(-1) !== receipt
        || priorWorkspace.state !== receipt.result.state || priorWorkspace.continuation)
        throw new Conflict('This historical Git continuation cannot advance the current workspace. Inspect its current state.');
      return receipt.result;
    }
    if (priorWorkspace?.state === 'continuing') throw new Conflict('Git continuation ownership is unverified; retain the workspace for operator inspection.');
    const refresh = this.activeConflict(owner.runId);
    if (!refresh || refresh.requestId !== requestId || !refresh.resolutionWorkspace) throw new Conflict('No active target conflict is assigned to this writer.');
    const workspace = refresh.resolutionWorkspace;
    if (workspace.state === 'ready') {
      await this.validateConflictWorkspace(owner.runId);
      return { state: 'ready' as const, head: workspace.toHead, conflicts: [] as string[] };
    }
    if (workspace.state !== 'conflicted') throw new Conflict('The target conflict requires an operator decision.');
    let work: RepositoryWorkRecord;
    try { work = (await this.conflictWork(owner.runId)).work; }
    catch (error) {
      const unresolved = this.unresolvedConflict(owner.runId);
      if (unresolved?.requestId === requestId && unresolved.resolutionWorkspace?.state === 'conflicted') {
        this.store.recordTargetRefreshWorkspace(owner.runId, requestId, { ...unresolved.resolutionWorkspace, state: 'operator_required' });
        return { state: 'operator_required' as const, conflicts: unresolved.conflicts ?? [] };
      }
      throw error;
    }
    const rebasing = await this.rebaseInProgress(work);
    if (rebasing) {
      const unmerged = (await this.git(work.path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort();
      if (!await this.conflictPathsWithinScope(work, refresh, unmerged)) {
        this.store.recordTargetRefreshWorkspace(owner.runId, requestId, { ...workspace, state: 'operator_required' }, unmerged);
        return { state: 'operator_required' as const, conflicts: unmerged };
      }
      for (const name of unmerged) {
        const file = await this.readConflict(owner.runId, name);
        if (/^<<<<<<<(?: .*)?\r?\n[\s\S]*?^=======\r?\n[\s\S]*?^>>>>>>>(?: .*)?(?:\r?\n|$)/m.test(file.content)) throw new Conflict('Resolve every conflict marker before continuing the target rebase.');
      }
      const identity = { attemptId, toolCallId, requestId, path: workspace.path, branch: workspace.branch, sourceHead: refresh.fromHead, paths: unmerged };
      this.store.beginTargetConflictContinuation(owner.runId, requestId, identity);
      const stillOwns = () => {
        this.currentWriter(attemptId);
        const pending = this.unresolvedConflict(owner.runId)?.resolutionWorkspace;
        if (pending?.state !== 'continuing' || JSON.stringify(pending.continuation) !== JSON.stringify(identity))
          throw new Conflict('Git continuation identity changed before dispatch.');
      };
      try {
        await this.git(work.path, ['add', '--', ...unmerged], false, stillOwns);
        try { await this.git(work.path, ['-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', '-c', 'commit.gpgSign=false', '-c', 'core.editor=true', 'rebase', '--continue'], false, stillOwns); }
        catch (error) { if (!this.gitReturned(error)) throw error; }
        await this.conflictWork(owner.runId, requestId, true);
        const remaining = await this.rebaseInProgress(work);
        const conflicts = remaining ? (await this.git(work.path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort() : [];
        if (remaining) {
          const result = { state: await this.conflictPathsWithinScope(work, refresh, conflicts) ? 'conflicted' as const : 'operator_required' as const, conflicts };
          this.store.finishTargetConflictContinuation(owner.runId, requestId, identity, result);
          return result;
        }
        const head = await this.ref(work.path, 'HEAD');
        let containsTarget = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', refresh.toBase, head]); } catch { containsTarget = false; }
        const names = async (from: string, to: string) => (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean).sort().join('\0');
        const instructionsHash = hash(JSON.stringify(await this.currentInstructions(work)));
        const expectedInstructionsHash = workspace.instructionsHash ?? hash(JSON.stringify(owner.instructions));
        const valid = containsTarget && await names(refresh.fromBase, refresh.fromHead) === await names(refresh.toBase, head)
          && !(await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all'])) && instructionsHash === expectedInstructionsHash;
        const result = valid ? { state: 'ready' as const, head, conflicts: [] as string[] }
          : { state: 'operator_required' as const, head, conflicts: [] as string[] };
        this.store.finishTargetConflictContinuation(owner.runId, requestId, identity, result);
        return result;
      } catch (error) {
        const run = this.store.getRun(owner.runId);
        if (!['cancelled', 'completed', 'closed'].includes(run.status) && run.desiredState !== 'cancelled')
          this.store.requireAttention(owner.runId, 'Isolated Git continuation ownership is unverified; retain the workspace and inspect before resuming.');
        throw error;
      }
    }
    const head = await this.ref(work.path, 'HEAD');
    let containsTarget = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', refresh.toBase, head]); } catch { containsTarget = false; }
    const names = async (from: string, to: string) => (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean).sort().join('\0');
    const instructionsHash = hash(JSON.stringify(await this.currentInstructions(work)));
    const expectedInstructionsHash = workspace.instructionsHash ?? hash(JSON.stringify(owner.instructions));
    if (!containsTarget || await names(refresh.fromBase, refresh.fromHead) !== await names(refresh.toBase, head)
      || await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all']) || instructionsHash !== expectedInstructionsHash) {
      this.store.recordTargetRefreshWorkspace(owner.runId, requestId, { ...workspace, state: 'operator_required', toHead: head }, []);
      return { state: 'operator_required' as const, conflicts: [] as string[] };
    }
    this.store.recordTargetRefreshWorkspace(owner.runId, requestId, { ...workspace, state: 'ready', toHead: head }, refresh.conflicts ?? []);
    return { state: 'ready' as const, head, conflicts: [] as string[] };
  }
  async readFile(runId: string, name: string) {
    if (this.activeConflict(runId)) return this.readIsolatedFile(runId, name, false);
    if (this.unresolvedConflict(runId) && !this.activeConflict(runId)) throw new Conflict('Git ownership is unverified; original worktree reads require operator inspection.');
    const work = this.store.repositoryWork(runId); if (!work) throw new NotFound('Repository work has not been prepared.');
    await this.validate(work); const path = this.scopedPath(work, name);
    if (!(await this.files(work)).some(file => file.path === name && file.hash !== 'deleted')) throw new NotFound('File is absent or ignored by the repository.');
    const stat = await lstat(path); if (stat.size > 1024 * 1024) throw new Conflict('File exceeds the text tool limit.');
    const content = await readFile(path, 'utf8'); return { path: name, content, sha256: hash(content) };
  }
  async listFiles(runId: string) {
    const conflict = this.activeConflict(runId);
    if (this.unresolvedConflict(runId) && !conflict) throw new Conflict('Git ownership is unverified; original worktree reads require operator inspection.');
    const work = conflict ? (await this.conflictWork(runId)).work : this.store.repositoryWork(runId);
    if (!work) throw new NotFound('Repository work has not been prepared.');
    if (!conflict) await this.validate(work);
    return this.files(work);
  }
  /**
   * Search tracked and non-ignored files in the owned worktree, or the tree of a pinned candidate
   * revision. Literal by default; bounded to 100 matches of at most 300 characters each.
   */
  async search(runId: string, input: unknown, revision?: RepositoryRevision) {
    const value = z.object({ query: z.string().min(1).max(500), regex: z.boolean().default(false), ignoreCase: z.boolean().default(false), path: z.string().max(1000).optional() }).strict().parse(input);
    if (value.path) { if (value.path.startsWith(':')) throw new Conflict('Search paths are plain repository paths.'); this.validateName(value.path.replace(/\/$/, '')); }
    const conflict = !revision ? this.activeConflict(runId) : undefined;
    if (!revision && this.unresolvedConflict(runId) && !conflict) throw new Conflict('Git ownership is unverified; original worktree search requires operator inspection.');
    const work = revision ? await this.pinnedTree(runId, revision) : conflict ? (await this.conflictWork(runId)).work : this.store.repositoryWork(runId);
    if (!work) throw new NotFound('Repository work has not been prepared.');
    if (!revision && !conflict) await this.validate(work);
    // Fixed output format regardless of repository grep configuration; one match past the global bound per file reveals truncation.
    const args = ['grep', '-z', '-n', '--no-column', '--full-name', '-I', '--no-color', '--max-count=101', ...(value.regex ? ['-E'] : ['-F']), ...(value.ignoreCase ? ['-i'] : []), ...(revision ? [] : ['--untracked']),
      '-e', value.query, ...(revision ? [revision.head] : []), '--', ...(value.path ? [`:(literal)${value.path}`] : [])];
    let output = '';
    // git grep exits 1 when nothing matches.
    try { output = await this.git(work.path, args, true); }
    catch (error) { const cause = error instanceof Conflict ? error.cause : undefined; if (!cause || typeof cause !== 'object' || !('code' in cause) || cause.code !== 1) throw new Conflict('Search failed. Narrow the query or path.', { cause: error }); }
    const prefix = revision ? `${revision.head}:` : '';
    const lines = output.split('\n').filter(Boolean);
    const matches = lines.slice(0, 100).map(line => {
      const [file, number, ...rest] = line.split('\0');
      return { path: file.startsWith(prefix) ? file.slice(prefix.length) : file, line: Number(number), text: rest.join('\0').slice(0, 300) };
    });
    return { query: value.query, regex: value.regex, path: value.path, revision: revision?.head, matches, truncated: lines.length > 100 };
  }
  private currentWriter(attemptId: string) {
    const attempt = this.store.getAttempt(attemptId); const work = this.store.repositoryWork(attempt.runId); const run = this.store.getRun(attempt.runId);
    if (!work || work.writer?.attemptId !== attemptId || attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running'
      || !this.store.runtime(run.id).assignments.some(item => item.currentAttemptId === attemptId)) throw new Conflict('Only the current writer may edit repository files.');
    return work;
  }
  async writeFile(attemptId: string, input: unknown) {
    const value = z.object({ path: z.string().max(1000), expectedHash: z.string().regex(/^[a-f0-9]{64}$/).nullable(), content: z.string().max(1024 * 1024) }).strict().parse(input);
    const owner = this.currentWriter(attemptId);
    const conflict = this.activeConflict(owner.runId);
    const unresolved = this.unresolvedConflict(owner.runId);
    if (unresolved && !conflict) throw new Conflict('Target conflict requires an operator decision; the original branch is not writable.');
    const isolated = conflict ? await this.conflictWork(owner.runId) : undefined;
    if (conflict && isolated?.refresh.requestId !== conflict.requestId) throw new Conflict('Conflict workspace changed.');
    const work = isolated?.work ?? owner;
    const current = await this.status(owner.runId);
    if (current.drift.requirements || current.drift.instructions || current.drift.head || (conflict && current.revision.dirty)
      || !await this.validatedConflictBaseDrift(owner.runId, current)) throw new Conflict('Repository base, head, requirements or instructions changed.');
    if (conflict && !conflict.conflicts?.includes(value.path)) throw new Conflict('Only recorded conflict files are in the resolution scope.');
    this.scopedPath(work, value.path);
    if (conflict && ['AGENTS.md', 'CLAUDE.md'].includes(basename(value.path))) throw new Conflict('Project instruction conflicts require operator review.');
    if (conflict?.resolutionWorkspace?.state === 'ready') throw new Conflict('Conflict rebase is ready; submit completion instead of editing again.');
    let ignored = false;
    try { await this.git(work.path, ['check-ignore', '--quiet', '--no-index', '--', value.path]); ignored = true; }
    catch (error) { const cause = error instanceof Conflict ? error.cause : undefined; if (!cause || typeof cause !== 'object' || !('code' in cause) || cause.code !== 1) throw error; }
    if (ignored) throw new Conflict('Edits to ignored files are outside the recorded source scope.');
    const latestWorkspace = conflict ? await this.conflictWork(owner.runId) : undefined;
    if (conflict && (latestWorkspace?.refresh.requestId !== conflict.requestId || latestWorkspace.refresh.resolutionWorkspace?.state !== 'conflicted' || latestWorkspace.work.path !== work.path))
      throw new Conflict('Conflict workspace changed.');
    // No await follows this ownership and scope recheck before the synchronous file replacement.
    this.currentWriter(attemptId);
    if (conflict) {
      const latest = this.activeConflict(owner.runId);
      if (latest?.requestId !== conflict.requestId || latest.resolutionWorkspace?.state !== 'conflicted' || latest.resolutionWorkspace.path !== work.path) throw new Conflict('Conflict workspace changed.');
    }
    if (this.requirementsHash(owner) !== owner.requirementsHash) throw new Conflict('Repository base, head, requirements or instructions changed.');
    const path = this.scopedPath(work, value.path);
    const before = existsSync(path) ? hash(readFileSync(path)) : null;
    if (before !== value.expectedHash) throw new Conflict('File changed since it was read.');
    const mode = existsSync(path) ? lstatSync(path).mode & 0o777 : 0o644;
    mkdirSync(dirname(path), { recursive: true });
    // The temporary file is outside the worktree, so interruption cannot stage it as source.
    const temporary = join(this.root, `.edit-${randomUUID()}`);
    writeFileSync(temporary, value.content, { flag: 'wx', mode, flush: true }); renameSync(temporary, path);
    return { path: value.path, sha256: hash(value.content) };
  }

  async claimWriter(attemptId: string, claim?: ImplementationClaim) {
    const attempt = this.store.getAttempt(attemptId); const work = await this.status(attempt.runId);
    if (this.unresolvedConflict(attempt.runId) && !this.activeConflict(attempt.runId)) throw new Conflict('Target conflict requires an operator decision; repository writes are held.');
    if (Object.entries(work.drift).some(([key, changed]) => key !== 'base' && changed)
      || !await this.validatedConflictBaseDrift(attempt.runId, work) || (this.activeConflict(attempt.runId) && work.revision.dirty))
      throw new Conflict('Repository or requirements changed. Reconcile drift before writing.');
    const checkpoint = await this.checkpoint(work, 'writer_started', attemptId);
    const latest = await this.status(attempt.runId);
    if (Object.entries(latest.drift).some(([key, changed]) => key !== 'base' && changed)
      || !await this.validatedConflictBaseDrift(attempt.runId, latest) || (this.activeConflict(attempt.runId) && latest.revision.dirty))
      throw new Conflict('Repository or requirements changed. Reconcile drift before writing.');
    return this.store.claimRepositoryWriter(attemptId, checkpoint, claim);
  }
  async releaseWriter(attemptId: string) {
    const attempt = this.store.getAttempt(attemptId); const work = this.store.repositoryWork(attempt.runId);
    if (!work) throw new NotFound('Repository work has not been prepared.');
    return this.store.releaseRepositoryWriter(attemptId, await this.checkpoint(work, 'interrupted', attemptId));
  }
  async reconcileStopped(runId: string) {
    const work = this.store.repositoryWork(runId); if (!work?.writer) return work;
    return this.releaseWriter(work.writer.attemptId);
  }
  /**
   * Verify the fetched target against a clean owned candidate. An unchanged operator retry records its
   * completed decision without touching Git; a moved target records intent before rebasing the branch.
   * A conflict, dropped change or interruption leaves the original branch at its previous head.
   */
  async refreshBase(runId: string, requestId: string, targetRef: string, sourceCandidateId: string, supersedes?: TargetRefresh['supersedes']) {
    const current = await this.status(runId);
    const previous = current.refreshes?.find(item => item.requestId === requestId);
    if (previous) return { refresh: previous, work: this.store.repositoryWork(runId)! };
    const source = this.store.codingRecord(runId)?.candidates.at(-1);
    if (!source || source.id !== sourceCandidateId || source.revision.head !== current.revision.head
      || source.revision.base !== current.base || current.writer || current.revision.dirty || current.drift.requirements || current.drift.instructions)
      throw new Conflict('The recorded candidate must be the clean, stopped head, with accepted requirements, before refreshing from the target.');
    const toBase = await this.ref(current.repository.path, targetRef);
    if (toBase === current.base) {
      const refresh: TargetRefresh = { requestId, at: new Date().toISOString(), fromBase: current.base, fromHead: current.revision.head,
        targetRef, toBase, toHead: current.revision.head, state: 'completed', sourceCandidateId, candidateId: sourceCandidateId,
        ...(supersedes ? { supersedes } : {}) };
      return { refresh, work: this.store.beginTargetRefresh(runId, refresh) };
    }
    try { await this.git(current.path, ['merge-base', '--is-ancestor', current.base, toBase]); }
    catch { throw new Conflict('The target no longer contains the validated base. It was rewritten; decide how to continue before refreshing.'); }
    let drivers = '';
    try { drivers = await this.git(current.path, ['config', '--get-regexp', '^merge\\..*\\.driver$']); } catch {}
    if (drivers) throw new Conflict('The repository configures custom merge drivers. Crewroom does not run them; refresh the branch manually.');
    const fromHead = current.revision.head;
    const refresh: TargetRefresh = { requestId, at: new Date().toISOString(), fromBase: current.base, fromHead, targetRef, toBase,
      sourceCandidateId, state: 'rebasing', ...(supersedes ? { supersedes } : {}) };
    this.store.beginTargetRefresh(runId, refresh);
    const work = this.store.repositoryWork(runId)!;
    try {
      await this.git(work.path, ['-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', '-c', 'commit.gpgSign=false', '-c', 'rebase.autoStash=false', '-c', 'rebase.autoSquash=false', '-c', 'rebase.updateRefs=false',
        'rebase', '--no-rerere-autoupdate', '--onto', toBase, refresh.fromBase]);
    } catch (error) {
      if (!this.gitReturned(error)) throw error;
      this.store.recordTargetRebaseRecovery(runId, requestId, 'rebase_stopped');
      let conflicts: string[] = [];
      try { conflicts = (await this.git(work.path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort(); } catch {}
      await this.undoRefresh(work, fromHead, refresh);
      if (!conflicts.length) {
        this.store.finishTargetRefresh(runId, requestId, { state: 'conflicted', conflicts });
        throw new Conflict('Rebasing onto the target failed. The branch is unchanged.');
      }
      const isolated = await this.createConflictWorkspace(work, refresh, conflicts);
      const saved = this.store.finishTargetRefresh(runId, requestId, { state: 'conflicted', conflicts: isolated.conflicts });
      return { refresh: saved.refreshes!.find(item => item.requestId === requestId)!, work: saved };
    }
    this.store.recordTargetRebaseRecovery(runId, requestId, 'rebase_stopped');
    return this.completeRefresh(work, refresh);
  }
  private async createConflictWorkspace(work: RepositoryWorkRecord, refresh: TargetRefresh, conflicts: string[]) {
    const parent = realpathSync(dirname(work.path));
    const path = join(parent, `${basename(work.path)}-refresh-${refresh.requestId}`);
    const branch = `${work.branch}-refresh-${refresh.requestId}`;
    let workspace: TargetRefreshWorkspace = { path, branch, state: 'preparing', requirementsHash: work.requirementsHash, instructionsHash: hash(JSON.stringify(work.instructions)) };
    this.store.recordTargetRefreshWorkspace(work.runId, refresh.requestId, workspace, conflicts);
    await this.git(work.repository.path, ['check-ref-format', `refs/heads/${branch}`]);
    if (existsSync(path) || (await this.branches(work.repository.path)).some(item => item.branch === `refs/heads/${branch}`)) throw new Conflict('Target conflict workspace already exists without a completed record. Inspect before retrying.');
    await this.git(work.repository.path, ['worktree', 'add', '-b', branch, path, refresh.fromHead]);
    const resolution = { ...work, path, branch, base: refresh.fromBase, startHead: refresh.fromHead, targetRef: refresh.targetRef, gitFile: undefined };
    const gitFile = await this.validate(resolution);
    workspace = { ...workspace, gitFile };
    this.store.recordTargetRefreshWorkspace(work.runId, refresh.requestId, workspace);
    try {
      await this.git(path, ['-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', '-c', 'commit.gpgSign=false', '-c', 'rebase.autoStash=false', '-c', 'rebase.autoSquash=false', '-c', 'rebase.updateRefs=false',
        'rebase', '--no-rerere-autoupdate', '--onto', refresh.toBase, refresh.fromBase]);
    } catch (error) {
      if (!this.gitReturned(error)) throw error;
      const isolatedConflicts = (await this.git(path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort();
      const sameConflicts = JSON.stringify(isolatedConflicts) === JSON.stringify(conflicts);
      const unchangedInstructions = hash(JSON.stringify(await this.currentInstructions(resolution))) === workspace.instructionsHash;
      const inAcceptedFileScope = unchangedInstructions && await this.conflictPathsWithinScope(resolution, refresh, isolatedConflicts);
      workspace = { ...workspace, state: sameConflicts && inAcceptedFileScope ? 'conflicted' : 'operator_required' };
      this.store.recordTargetRefreshWorkspace(work.runId, refresh.requestId, workspace);
      return { workspace, conflicts: isolatedConflicts.length ? isolatedConflicts : conflicts };
    }
    workspace = { ...workspace, state: 'operator_required' };
    this.store.recordTargetRefreshWorkspace(work.runId, refresh.requestId, workspace);
    return { workspace, conflicts: [] };
  }
  private async completeRefresh(work: RepositoryWorkRecord, refresh: TargetRefresh) {
    const moved = { ...work, base: refresh.toBase, targetRef: refresh.targetRef };
    const head = await this.ref(work.path, 'HEAD');
    const names = async (from: string, to: string) => (await this.git(work.path, ['diff', '--name-only', '--no-renames', '-z', from, to, '--'])).split('\0').filter(Boolean).sort().join('\0');
    let contained = true; try { await this.git(work.path, ['merge-base', '--is-ancestor', refresh.toBase, head]); } catch { contained = false; }
    const clean = !(await this.git(work.path, ['status', '--porcelain=v1', '--untracked-files=all']));
    if (!contained || !clean || await names(refresh.fromBase, refresh.fromHead) !== await names(refresh.toBase, head)) {
      await this.undoRefresh(work, refresh.fromHead, refresh);
      const saved = this.store.finishTargetRefresh(work.runId, refresh.requestId, { state: 'conflicted', conflicts: [] });
      return { refresh: saved.refreshes!.find(item => item.requestId === refresh.requestId)!, work: saved };
    }
    try {
      const observed = await this.verifyCurrentTarget(work, refresh);
      if (observed.state === 'changed') throw new Conflict('The live target changed before the refresh receipt.');
      const current = await this.status(work.runId);
      const source = this.store.codingRecord(work.runId)?.candidates.at(-1);
      if (current.revision.head !== head || current.revision.dirty || current.base !== refresh.fromBase || current.targetRef !== work.targetRef
        || current.path !== work.path || current.branch !== work.branch || (refresh.sourceCandidateId && source?.id !== refresh.sourceCandidateId)
        || source?.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase
        || current.drift.requirements || current.drift.instructions || this.store.getRun(work.runId).status !== 'paused'
        || current.writer || this.store.runtime(work.runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped'))
        throw new Conflict('Refresh source or ownership changed before completion.');
      const checkpoint = await this.checkpoint(moved, 'refreshed');
      const saved = this.store.finishTargetRefresh(work.runId, refresh.requestId, { state: 'completed', toHead: head }, checkpoint);
      this.observed.delete(work.runId);
      return { refresh: saved.refreshes!.find(item => item.requestId === refresh.requestId)!, work: saved };
    } catch (error) {
      // Never reset a branch that may have acquired a new owner or new source after the remote observation.
      const current = await this.status(work.runId).catch(() => undefined);
      const source = this.store.codingRecord(work.runId)?.candidates.at(-1);
      if (!current || current.writer || current.revision.head !== head || current.revision.dirty || current.path !== work.path
        || this.store.getRun(work.runId).status !== 'paused' || (refresh.sourceCandidateId && source?.id !== refresh.sourceCandidateId)
        || source?.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase
        || this.store.codingRecord(work.runId)?.completions?.some(item => item.state === 'requested')
        || this.store.runtime(work.runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw error;
      await this.undoRefresh(work, refresh.fromHead, refresh);
      this.store.finishTargetRefresh(work.runId, refresh.requestId, { state: 'conflicted', conflicts: [] });
      throw error;
    }
  }
  /** Abort an unfinished rebase and restore the owned branch to its recorded head. */
  private async undoRefresh(work: RepositoryWorkRecord, head: string, refresh?: TargetRefresh) {
    if (refresh) this.store.recordTargetRebaseRecovery(work.runId, refresh.requestId, 'undo_started');
    for (const name of ['rebase-merge', 'rebase-apply']) {
      if (existsSync(await this.git(work.path, ['rev-parse', '--path-format=absolute', '--git-path', name]))) { await this.git(work.path, ['rebase', '--abort']); break; }
    }
    if (await this.ref(work.path, 'HEAD') !== head) await this.git(work.path, ['reset', '--hard', '--no-recurse-submodules', head]);
    this.observed.delete(work.runId);
    if (refresh) {
      const restored = await this.status(work.runId);
      if (await this.rebaseInProgress(work) || restored.revision.head !== head || restored.revision.dirty
        || restored.path !== work.path || restored.branch !== work.branch || restored.base !== refresh.fromBase
        || restored.targetRef !== work.targetRef || restored.drift.requirements || restored.drift.instructions)
        throw new Conflict('Target rebase restoration is uncertain; retain its rebasing intent.');
      this.store.recordTargetRebaseRecovery(work.runId, refresh.requestId, 'restored');
    }
  }
  /** Reconcile partial Git transitions without discarding an isolated conflict or replaying an unknown promotion. */
  async reconcileRefresh(runId: string) {
    const work = this.store.repositoryWork(runId);
    if (work?.refreshes?.some(item => item.state === 'conflicted' && ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
      throw new Conflict('A retained workspace has unverified Git ownership; do not recover or clean another workspace.');
    const refresh = work?.refreshes?.findLast(item => item.state === 'rebasing' || (item.state === 'conflicted' && !item.supersededBy
      && (['preparing', 'continuing', 'promoting'].includes(item.resolutionWorkspace?.state ?? '') || (item.resolutionWorkspace?.state === 'ready'
        && this.store.codingRecord(runId)?.completions?.some(operation => operation.state === 'requested' && operation.conflictRefreshId === item.requestId))))
      || (item.state === 'completed' && item.resolutionWorkspace?.state === 'promoted'));
    if (!work || !refresh) return;
    if (refresh.state === 'completed') { await this.cleanupPromotedConflict(work, refresh); return; }
    if (refresh.state === 'conflicted') {
      const workspace = refresh.resolutionWorkspace!;
      if (['preparing', 'continuing'].includes(workspace.state)) throw new Conflict('Isolated Git mutation ownership is unverified; retain the workspace for inspection.');
      const completion = this.store.codingRecord(runId)?.completions?.find(item => item.state === 'requested'
        && item.conflictRefreshId === refresh.requestId && (workspace.state !== 'promoting' || item.id === workspace.operationId));
      if (!completion || !workspace.toHead) throw new Conflict('Conflict promotion lacks its recorded requested completion.');
      if (workspace.state === 'promoting') {
        const run = this.store.getRun(runId);
        if (!['paused', 'attention_required'].includes(run.status) || run.desiredState === 'cancelled' || work.writer
          || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped'))
          throw new Conflict('Stop agent ownership before reconciling a checked promotion.');
        try { await this.reconcilePromotion(work, refresh, completion); }
        catch (error) { if (!(error instanceof TargetChangedRejection)) throw error; }
        return;
      }
      const { work: isolated } = await this.conflictWork(runId, refresh.requestId);
      if (await this.rebaseInProgress(isolated) || await this.ref(isolated.path, 'HEAD') !== workspace.toHead
        || await this.git(isolated.path, ['status', '--porcelain=v1', '--untracked-files=all'])) throw new Conflict('Isolated promotion changed before recovery.');
      const stopped = () => {
        const run = this.store.getRun(runId);
        if (!['paused', 'attention_required'].includes(run.status) || run.desiredState === 'cancelled' || this.store.repositoryWork(runId)?.writer
          || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')) throw new Conflict('Stop ownership before recovering promotion.');
      };
      stopped();
      try {
        if (workspace.state === 'ready') {
          const live = await this.verifyCurrentTarget(work, refresh);
          if (live.state === 'changed') await this.rejectChangedConflict(runId, refresh, completion, live.observedTarget);
        }
      } catch (error) { if (!(error instanceof TargetChangedRejection)) throw error; }
      return;
    }
    if (!refresh.rebaseRecovery || refresh.rebaseRecovery === 'undo_started')
      throw new Conflict('Git rebase exit or rollback is unproved; retain its rebasing intent for inspection.');
    if (refresh.resolutionWorkspace?.state === 'preparing')
      throw new Conflict('Conflict workspace preparation may still own a Git child; inspect before retrying.');
    this.metadata(work);
    if (refresh.resolutionWorkspace) {
      if (refresh.rebaseRecovery !== 'restored') throw new Conflict('Conflict workspace recovery lacks a completed original rollback.');
      const original = await this.status(runId);
      if (original.revision.head !== refresh.fromHead || original.revision.dirty || original.drift.requirements || original.drift.instructions)
        throw new Conflict('Original branch changed during retained workspace recovery.');
      const workspace = refresh.resolutionWorkspace;
      if (!existsSync(workspace.path)) {
        if ((await this.branches(work.repository.path)).some(item => item.branch === `refs/heads/${workspace.branch}`)) throw new Conflict('Conflict workspace branch exists without its recorded directory.');
        if (!refresh.conflicts?.length) throw new Conflict('Interrupted conflict has no recorded paths.');
        const captured = await this.createConflictWorkspace(work, refresh, refresh.conflicts);
        this.store.finishTargetRefresh(runId, refresh.requestId, { state: 'conflicted', conflicts: captured.conflicts });
        return;
      }
      const isolated = { ...work, path: workspace.path, branch: workspace.branch, gitFile: workspace.gitFile };
      const gitFile = this.metadata(isolated);
      if (await this.commonDir(isolated.path) !== work.commonDir
        || !(await this.branches(work.repository.path)).some(item => item.worktree === isolated.path && (item.branch === `refs/heads/${workspace.branch}` || item.detached))
        || await this.ref(work.repository.path, `refs/heads/${workspace.branch}`) !== refresh.fromHead) throw new Conflict('Interrupted conflict workspace ownership changed.');
      if (!workspace.gitFile) this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, gitFile });
      const conflicts = (await this.git(isolated.path, ['diff', '--name-only', '--diff-filter=U', '-z'])).split('\0').filter(Boolean).sort();
      const expected = refresh.conflicts?.slice().sort() ?? [];
      const valid = await this.rebaseInProgress(isolated) && conflicts.length > 0 && JSON.stringify(conflicts) === JSON.stringify(expected);
      this.store.recordTargetRefreshWorkspace(runId, refresh.requestId, { ...workspace, gitFile, state: valid ? 'conflicted' : 'operator_required' });
      this.store.finishTargetRefresh(runId, refresh.requestId, { state: 'conflicted', conflicts: conflicts.length ? conflicts : expected });
      return;
    }
    if (refresh.rebaseRecovery === 'restored' && (await this.rebaseInProgress(work) || await this.ref(work.path, 'HEAD') !== refresh.fromHead))
      throw new Conflict('Recorded rebase restoration changed; retain its rebasing intent.');
    if (!(await this.rebaseInProgress(work)) && await this.ref(work.path, 'HEAD') !== refresh.fromHead) {
      try { await this.completeRefresh(work, refresh); return; } catch { return; }
    }
    {
      const run = this.store.getRun(runId);
      const source = this.store.codingRecord(runId)?.candidates.at(-1);
      if (run.status !== 'paused' || work.writer || work.base !== refresh.fromBase
        || !source || source.id !== refresh.sourceCandidateId || source.revision.head !== refresh.fromHead || source.revision.base !== refresh.fromBase
        || this.store.codingRecord(runId)?.completions?.some(item => item.state === 'requested')
        || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')
        || await this.ref(work.repository.path, `refs/heads/${work.branch}`) !== refresh.fromHead)
        throw new Conflict('Interrupted retry has uncertain source or ownership; retain its rebasing intent.');
    }
    if (refresh.rebaseRecovery !== 'restored') await this.undoRefresh(work, refresh.fromHead, refresh);
    {
      const restored = await this.status(runId);
      if (await this.rebaseInProgress(work) || restored.revision.head !== refresh.fromHead || restored.revision.dirty
        || restored.path !== work.path || restored.branch !== work.branch || restored.base !== refresh.fromBase
        || restored.targetRef !== work.targetRef || restored.drift.requirements || restored.drift.instructions)
        throw new Conflict('Interrupted retry restoration is uncertain; retain its rebasing intent.');
    }
    this.store.finishTargetRefresh(runId, refresh.requestId, { state: 'interrupted' });
  }
}
