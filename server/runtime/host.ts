import { appendFileSync, readFileSync, readdirSync, existsSync, renameSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { join } from 'node:path';
import { OmpRpc } from './rpc';
import { configureAssignment } from './model-policy';
import { ContainerCommand } from './container-command';
import { ProviderGateway } from './provider-gateway';
import { codingTools } from './coding-tools';
import { jiraTools } from './jira-tools';
import { publicationTools } from './publication-tools';
import { researchTools } from './research-tools';
import { ObservationStream } from './observation';
import { PaneFormatter } from './pane-format';
import { briefLimits, hostInputLimit } from './types';

const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const log = join(config.root, 'events.jsonl');
const inputs = join(config.root, 'inputs.jsonl');
const events: { sequence: number; event: any }[] = [];
const eventWaiters = new Set<() => void>();
async function eventPage(after: number, wait: number) {
  // Host sequence numbers begin at one and never skip: cursor maps directly to an index.
  const start = Math.max(0, Math.floor(after));
  const page = () => events.slice(start, start + 128);
  if (sequence > after || !wait || !accepting) return page();
  await new Promise<void>(resolve => {
    const done = () => { clearTimeout(timer); eventWaiters.delete(done); resolve(); };
    const timer = setTimeout(done, wait);
    eventWaiters.add(done);
    if (sequence > after || !accepting) done();
  });
  return page();
}
const observation = new ObservationStream();
// The tmux pane shows readable activity; the JSON event journal remains the durable record.
const pane = new PaneFormatter(text => { process.stdout.write(text); });
let sequence = 0;
let accepting = true;
const seen = new Map<string, string>();
const admitting = new Map<string, Promise<void>>();
const inputBodies = new Map<string, string>();
const toolRoots = new Map<string, string>();
const commands = new Map<string, ContainerCommand>();
async function stopTools() {
  const parent = join(config.root, 'tools');
  if (!existsSync(parent)) return;
  for (const name of readdirSync(parent)) {
    const root = join(parent, name);
    const job = commands.get(root) ?? new ContainerCommand(root, config.commandEngine);
    const result = await job.stop();
    if (result.state !== 'stopped') throw new Error(result.reason ?? 'Tool ownership is unresolved.');
    await job.remove();
  }
}
async function runTool(frame: any, prepared?: any) {
  const command = prepared ?? (config.tools ?? []).find((item: any) => item.id === frame.arguments?.commandId);
  if (!command || (!prepared && Object.keys(frame.arguments).some(key => key !== 'commandId'))) throw new Error('Command is not admitted.');
  const root = join(config.root, 'tools', createHash('sha256').update(frame.toolCallId).digest('hex'));
  const job = new ContainerCommand(root, config.commandEngine);
  commands.set(root, job);
  toolRoots.set(frame.id, root);
  const retained = existsSync(join(root, 'container.json'));
  if (!retained) await job.start(command);
  if (!accepting) { await job.stop(); throw new Error('Command stopped during admission.'); }
  observe({ type: retained ? 'tool_receipt_reused' : 'tool_started', toolCallId: frame.toolCallId, commandId: command.id, container: (await job.inspect()).id });
  // Live output is observation only: the completed receipt still comes from the engine's final logs.
  const live = join(root, 'live.log'); const following = new AbortController();
  void job.follow(chunk => {
    try { appendFileSync(live, chunk, { mode: 0o600 }); } catch {}
    try { pane.output(redact(chunk.toString())); } catch {}
  }, following.signal).catch(() => {});
  let ownership;
  try { ownership = await job.wait(); } finally { following.abort(); }
  const result = { exitCode: ownership.exitCode };
  observe({ type: 'tool_exited', toolCallId: frame.toolCallId, result, ownership });
  if (ownership.state !== 'stopped') throw new Error('Tool descendants have unresolved ownership.');
  const output = await job.logs();
  appendFileSync(join(root, 'output.log'), output, { mode: 0o600 });
  await job.remove();
  return { result, output: output.slice(-16000) };
}
async function backendTool(frame: any, endpoint: string, args = frame.arguments) {
  const response = await fetch(`${config.backend}/internal/${endpoint}`, { method: 'POST', signal: AbortSignal.timeout(30000),
    headers: { 'Content-Type': 'application/json', Origin: config.backend, Authorization: `Bearer ${config.token}` },
    body: JSON.stringify({ attemptId: config.attemptId, toolCallId: frame.toolCallId, arguments: args }) });
  const result = await response.json(); if (!response.ok) throw new Error(result.error ?? 'Tool operation was rejected.'); return result;
}

function redact(value: any): any {
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value)
    .filter(([key]) => !/^(apiKey|headers|authorization|token|providerPayload)$/i.test(key)).map(([key, item]) => [key, redact(item)]));
  if (typeof value === 'string') return config.secrets.reduce((text: string, secret: string) => secret ? text.split(secret).join('[redacted]') : text, value);
  return value;
}
function observe(event: any) {
  try { pane.frame(redact(event)); } catch { /* Pane rendering never affects durable observation. */ }
  const projected = observation.accept(event); if (!projected) return;
  const saved = { sequence: ++sequence, event: redact(projected) };
  appendFileSync(log, JSON.stringify(saved) + '\n', { mode: 0o600, flush: true });
  events.push(saved);
  for (const wake of eventWaiters) wake();
}
// Bind before OMP starts: its provider URL names this host's port, which may be OS-assigned.
let ready = false;
const server = createServer(async (req, res) => {
  res.setHeader('Content-Type', 'application/json');
  try {
    if (!ready) { res.writeHead(503); res.end(JSON.stringify({ error: 'Host is starting.' })); return; }
    if (req.url?.startsWith('/provider/')) { await provider.serve(req, res); return; }
    if (req.headers.authorization !== `Bearer ${config.token}` || req.method !== 'POST') { res.writeHead(403); res.end('{}'); return; }
    let raw = ''; for await (const chunk of req) { raw += chunk; if (raw.length > hostInputLimit) throw new Error('Request too large.'); }
    const body = JSON.parse(raw);
    if (body.op === 'identity') res.end(JSON.stringify({ hostPid: process.pid, ompPid: rpc.child.pid, ...identity }));
    else if (body.op === 'events') res.end(JSON.stringify(await eventPage(Number(body.after) || 0, Number.isFinite(body.wait) ? Math.max(0, Math.min(25000, body.wait)) : 0)));
    else if (body.op === 'state') res.end(JSON.stringify(redact(await rpc.command('get_state'))));
    else if (body.op === 'input_status') res.end(JSON.stringify({ disposition: seen.get(body.id) ?? (admitting.has(body.id) ? 'admitting' : 'absent') }));
    else if (body.op === 'input') {
      await acceptInput(body);
      res.end(JSON.stringify({ received: true }));
    } else if (body.op === 'close') {
      await stopExecution(); res.end(JSON.stringify({ stopped: true })); server.close();
    } else { res.writeHead(404); res.end('{}'); }
  } catch (error) { res.writeHead(409); res.end(JSON.stringify({ error: error instanceof Error ? error.message : 'Host operation failed.' })); }
});
await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(config.port ?? 0, '127.0.0.1', resolve); });
const port = (server.address() as AddressInfo).port;
for (const provider of Object.values(config.models.providers) as { baseUrl: string }[]) provider.baseUrl = `http://127.0.0.1:${port}${provider.baseUrl}`;
writeFileSync(join(config.env.PI_CODING_AGENT_DIR, 'models.yml'), JSON.stringify(config.models), { mode: 0o600 });
const rpc = new OmpRpc(config.command, config.args, { cwd: config.root, env: config.env }, frame => {
  if (frame.type === 'prompt_result' && ['completed', 'error'].includes(frame.status) && provider.failure) {
    const failure = provider.failure;
    frame = { ...frame, status: 'error', error: { message: frame.error?.message ?? `Provider ${failure.kind}; no usable final result was confirmed.`,
      kind: failure.kind, retryable: failure.retryable, provider: config.model.resolved.provider, model: config.model.resolved.id } };
  }
  observe(frame);
  if (frame.type === 'prompt_result' && seen.has(frame.id)) {
    seen.set(frame.id, frame.status);
    appendFileSync(inputs, JSON.stringify({ id: frame.id, disposition: frame.status }) + '\n', { mode: 0o600, flush: true });
  }
  if (frame.type === 'host_tool_cancel') {
    const root = toolRoots.get(frame.targetId);
    if (root) void commands.get(root)!.stop().then(result => observe({ type: 'tool_cancelled', targetId: frame.targetId, ownership: result }));
  }
  if (frame.type === 'host_tool_call') void (async () => {
    try {
      if (!accepting) throw new Error('Host tool is unavailable.');
      if (frame.toolName === 'run_command') {
        const result = await runTool(frame);
        rpc.send({ type: 'host_tool_result', id: frame.id, result: { content: [{ type: 'text', text: JSON.stringify(result) }] } });
        return;
      }
      if (frame.toolName === 'run_check' && config.coding && config.commandEngine) {
        const prepared = await backendTool(frame, 'coding/check-prepare');
        const execution = await runTool(frame, { ...prepared.spec, id: frame.arguments.checkId });
        const result = await backendTool(frame, 'coding/check-finish', {});
        rpc.send({ type: 'host_tool_result', id: frame.id, result: { content: [{ type: 'text', text: JSON.stringify({ ...result, outputPreview: execution.output }) }] } });
        return;
      }
      if (frame.toolName === 'run_shell' && config.coding && config.commandEngine && !config.coordinator) {
        const prepared = await backendTool(frame, 'coding/shell');
        const execution = await runTool(frame, { ...prepared.spec, id: 'shell' });
        rpc.send({ type: 'host_tool_result', id: frame.id, result: { content: [{ type: 'text', text: JSON.stringify({ ...execution, note: 'Ad-hoc output over a read-only worktree. It is not check evidence; use run_check for required checks.' }) }] } });
        return;
      }
      const endpoints: Record<string, string> = { ...(config.publication ? { publication_status: 'publication/status', ...(config.coordinator ? { publish_change: 'publication/prepare' } : {}) } : {}), ...(config.jira ? { jira_updates: 'jira/updates', ...(config.reporter ? { jira_report: 'jira/report' } : {}) } : {}), send_message: 'agent-message', steering_receipt: 'steering-receipt', ...(config.refinement ? { propose_refinement: 'refinement-proposal' } : { list_roles: 'agent-roles', propose_team: 'team-proposal' }), ...(config.coding ? { coding_status: 'coding/status', repository_read: 'coding/read', claim_implementation: 'coding/claim', write_file: 'coding/edit', finish_implementation: 'coding/complete', submit_review: 'coding/review', assign_review: 'coding/review-assign' } : {}) };
      if (config.coding) Object.assign(endpoints, { check_output: 'coding/output', repository_search: 'coding/search', continue_target_conflict: 'coding/conflict-continue', ...(config.coordinator ? {} : { edit_file: 'coding/patch' }) });
      if (config.research) Object.assign(endpoints, { submit_research_artifact: 'research/submit', ...(config.researchRepository ? { repository_read: 'research/read', repository_search: 'research/search' } : {}), ...(config.coordinator ? { request_research_completion: 'research/complete' } : {}) });
      if (config.jira && config.researchCoordinatorReporter) endpoints.jira_report = 'jira/report';
      if (config.coordinator && !config.refinement && !config.research) endpoints.propose_learning = 'learning-proposal';
      const endpoint = endpoints[frame.toolName];
      if (!endpoint) throw new Error('Host tool is unavailable.');
      const result = await backendTool(frame, endpoint);
      rpc.send({ type: 'host_tool_result', id: frame.id, result: { content: [{ type: 'text', text: JSON.stringify(result) }] } });
    } catch (error) {
      rpc.send({ type: 'host_tool_result', id: frame.id, isError: true, result: { content: [{ type: 'text', text: error instanceof Error ? error.message : 'Tool outcome unknown.' }] } });
    }
  })();
});
let recoveryMode: 'session' | 'replacement' | undefined = config.previousAttemptId ? 'replacement' : undefined;
let sessionDirectory = join(config.root, 'sessions');
if (config.resumeSessionDir) {
  const restored: any = await rpc.command('open_session', { sessionDir: config.resumeSessionDir });
  const page: any = await rpc.command('get_messages_page', { limit: 1 });
  observe({ type: 'session_restore_inspected', restored, messageCount: page.totalMessages ?? page.messages?.length });
  if (restored.resumed && restored.sessionId === config.previousSessionId && page.messages?.length) {
    recoveryMode = 'session'; sessionDirectory = config.resumeSessionDir;
  } else await rpc.command('new_session');
}
const identity = { ...await configureAssignment(rpc, config.model).catch(async error => { await rpc.close(); throw error; }), recoveryMode, sessionDirectory };
await rpc.command('set_host_tools', { tools: [{ name: 'send_message', label: 'Send external message', loadMode: 'essential',
  description: 'Send a scoped message through Rust Agent Mail. A storage receipt does not mean completed work.',
  parameters: { type: 'object', properties: { to: { type: 'string', enum: config.recipients }, body: { type: 'string' } }, required: ['to', 'body'], additionalProperties: false } },
  ...(config.coordinator && !config.refinement ? [{ name: 'list_roles', label: 'Inspect available roles', loadMode: 'essential', description: 'Read the current user role catalog. Select roles by their purpose; configured models are validated by Crewroom.', parameters: { type: 'object', properties: {}, additionalProperties: false } },
    { name: 'propose_team', label: 'Propose specialists', loadMode: 'essential', description: 'Propose only the specialists this work needs. Give each a structured brief: the accepted criteria it owns (zero-based index), its obligation for each and the configured check IDs that verify it, plus the teammates whose results it depends on. Crewroom validates capacity, models and briefs, then applies the team at a safe stopping point. Retire finished specialists to free active slots; retain their evidence and history. A pending proposal has no executing agents.',
      parameters: { type: 'object', properties: { specialists: { type: 'array', minItems: 0, maxItems: 3, items: { type: 'object', properties: { name: { type: 'string' }, roleId: { type: 'string' }, input: { type: 'string' },
        criteria: { type: 'array', items: { type: 'object', properties: { index: { type: 'integer', minimum: 0 }, obligation: { type: 'string' }, checkIds: { type: 'array', maxItems: briefLimits.checkIds, items: { type: 'string' } } }, required: ['index', 'obligation'], additionalProperties: false } },
        dependsOn: { type: 'array', maxItems: briefLimits.dependsOn, items: { type: 'string' } } }, required: ['name', 'roleId', 'input'], additionalProperties: false } }, retire: { type: 'array', maxItems: 3, items: { type: 'object', properties: { name: { type: 'string' }, reason: { type: 'string' } }, required: ['name', 'reason'], additionalProperties: false } } }, required: ['specialists'], additionalProperties: false } },
    { name: 'steering_receipt', label: 'Reconcile steering', loadMode: 'essential',
    description: 'Report that an operator instruction was applied to the work plan or rejected with a reason. This is not proof of completed code.',
    parameters: { type: 'object', properties: { messageId: { type: 'string' }, disposition: { type: 'string', enum: ['applied', 'rejected'] }, reason: { type: 'string' } }, required: ['messageId', 'disposition', 'reason'], additionalProperties: false } },
    ...(config.research ? [] : [{ name: 'propose_learning', label: 'Propose a learning update', loadMode: 'essential',
      description: 'Propose replacement instructions for a reusable role, or replacement project guidance for this repository, citing recorded run evidence (check, review, steering or check_policy IDs). The operator approves or rejects it; approval creates a new version for future assignments only. It cannot change models, checks or readiness policy.',
      parameters: { type: 'object', properties: { target: { type: 'object', properties: { kind: { type: 'string', enum: ['role', 'project'] }, roleId: { type: 'string' } }, required: ['kind'], additionalProperties: false },
        change: { type: 'string' }, rationale: { type: 'string' }, evidence: { type: 'array', minItems: 1, maxItems: 20, items: { type: 'object', properties: { kind: { type: 'string', enum: ['check', 'review', 'steering', 'check_policy'] }, id: { type: 'string' } }, required: ['kind', 'id'], additionalProperties: false } } },
        required: ['target', 'change', 'rationale', 'evidence'], additionalProperties: false } }]) ] : []),
  ...(config.refinement ? [
    { name: 'steering_receipt', label: 'Reconcile steering', loadMode: 'essential', description: 'Report an operator message as applied or rejected with a reason.', parameters: { type: 'object', properties: { messageId: { type: 'string' }, disposition: { type: 'string', enum: ['applied', 'rejected'] }, reason: { type: 'string' } }, required: ['messageId', 'disposition', 'reason'], additionalProperties: false } },
    { name: 'propose_refinement', label: 'Propose requirements', loadMode: 'essential', description: 'Propose a brief and testable criteria, recording decisions and unresolved questions separately. The operator accepts or discards; proposing never launches coding work.', parameters: { type: 'object', properties: { brief: { type: 'string' }, criteria: { type: 'array', minItems: 1, maxItems: 100, items: { type: 'string' } }, decisions: { type: 'array', maxItems: 100, items: { type: 'string' } }, unresolvedQuestions: { type: 'array', maxItems: 100, items: { type: 'string' } } }, required: ['brief', 'criteria', 'decisions', 'unresolvedQuestions'], additionalProperties: false } }
  ] : []),
  ...(config.publication ? publicationTools(config.coordinator) : []),
  ...(config.jira ? jiraTools(config.reporter || config.researchCoordinatorReporter, config.research, config.researchCoordinatorReporter) : []),
  ...(config.coding ? codingTools(config.coordinator, !!config.commandEngine) : []),
  ...(config.research ? researchTools(config.coordinator, config.researchRepository) : []),
  ...(config.tools?.length ? [{ name: 'run_command', label: 'Run admitted command', loadMode: 'essential', description: 'Run one configured command under process ownership.',
    parameters: { type: 'object', properties: { commandId: { type: 'string', enum: config.tools.map((item: any) => item.id) } }, required: ['commandId'], additionalProperties: false } }] : [])] }).catch(async error => { await rpc.close(); throw error; });
observe({ type: 'host_ready', hostPid: process.pid, ompPid: rpc.child.pid, ...identity });
const provider = new ProviderGateway(config.root, config.model, config.upstreamProvider, config.providerToken, observe, config.budgetRoot);
ready = true;
// Published atomically only once the host can answer; the supervisor then verifies identity.
writeFileSync(join(config.root, 'endpoint.json.tmp'), JSON.stringify({ port }), { mode: 0o600 });
renameSync(join(config.root, 'endpoint.json.tmp'), join(config.root, 'endpoint.json'));
async function stopExecution() {
  accepting = false;
  for (const wake of eventWaiters) wake();
  provider.close();
  await Promise.race([rpc.command('abort').catch(() => {}), new Promise(resolve => setTimeout(resolve, 500))]);
  await stopTools(); await rpc.close();
}
async function acceptInput(body: any) {
  if (typeof body.id !== 'string' || !body.id || typeof body.message !== 'string') throw new Error('Input identity and message are required.');
  if (inputBodies.has(body.id) && inputBodies.get(body.id) !== body.message) throw new Error('Input identity has different content.');
  inputBodies.set(body.id, body.message);
  const pending = admitting.get(body.id); if (pending) return pending;
  if (!accepting) throw new Error('Host is stopping.');
  if (seen.has(body.id)) {
    if (seen.get(body.id) === 'accepted') throw new Error('Input dispatch outcome is uncertain.');
    return;
  }
  const work = (async () => {
    const state = await rpc.command('get_state');
    if (!accepting) throw new Error('Host is stopping.');
    if (state.isCompacting || (state.isStreaming && body.mode !== 'steer')) throw new Error('Agent is busy.');
    appendFileSync(inputs, JSON.stringify({ id: body.id, message: body.message, disposition: 'accepted' }) + '\n', { mode: 0o600, flush: true });
    seen.set(body.id, 'accepted');
    await rpc.command('prompt', { message: body.message, ...(body.mode === 'steer' ? { streamingBehavior: 'steer' } : {}) }, body.id);
    if (seen.get(body.id) === 'accepted') {
      seen.set(body.id, 'received');
      appendFileSync(inputs, JSON.stringify({ id: body.id, disposition: 'received' }) + '\n', { flush: true });
    }
  })();
  admitting.set(body.id, work);
  try { await work; } finally { admitting.delete(body.id); }
}
for (const signal of ['SIGTERM', 'SIGINT'] as const) process.on(signal, async () => {
  try { await stopExecution(); server.close(); }
  catch (error) { observe({ type: 'ownership_unresolved', reason: String(error) }); }
});
