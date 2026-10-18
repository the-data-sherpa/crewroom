/**
 * Renders an agent host's activity as readable terminal output for its tmux pane.
 * The pane is an observation surface only: durable events, receipts and evidence
 * stay in the workflow store, and nothing parses this output.
 */
const reset = '\x1b[0m';
const style = { dim: '\x1b[2m', bold: '\x1b[1m', red: '\x1b[31m', green: '\x1b[32m', yellow: '\x1b[33m', blue: '\x1b[34m', magenta: '\x1b[35m', cyan: '\x1b[36m', italic: '\x1b[3m' };
type Frame = Record<string, any>;

export function summarizeArguments(value: unknown, limit = 90): string {
  if (!value || typeof value !== 'object') return '';
  const args = value as Record<string, unknown>;
  const preferred = ['path', 'checkId', 'commandId', 'to', 'name', 'messageId', 'disposition'];
  const parts = preferred.filter(key => typeof args[key] === 'string' || typeof args[key] === 'number').map(key => `${key}=${String(args[key])}`);
  if (typeof args.content === 'string') parts.push(`${args.content.split('\n').length} lines`);
  if (Array.isArray(args.specialists)) parts.push(`+${args.specialists.map((item: any) => item?.name ?? '?').join(', +')}`);
  if (!parts.length) parts.push(Object.keys(args).slice(0, 4).join(' '));
  const text = parts.join(' ');
  return text.length > limit ? `${text.slice(0, limit - 1)}…` : text;
}

export class PaneFormatter {
  private streaming?: 'text' | 'thinking';
  constructor(private out: (text: string) => void, private now: () => Date = () => new Date()) {}

  private stamp() { return `${style.dim}${this.now().toTimeString().slice(0, 8)}${reset}`; }
  private endStream() { if (this.streaming) { this.out(`${reset}\n`); this.streaming = undefined; } }
  line(color: string, text: string) { this.endStream(); this.out(`${this.stamp()}  ${color}${text}${reset}\n`); }

  /** Raw output from a running command, shown with a gutter so it reads as nested. */
  output(chunk: string) {
    this.endStream();
    const body = chunk.replace(/\r?\n(?!$)/g, `\n${style.dim}│${reset} `);
    this.out(`${style.dim}│${reset} ${body}${body.endsWith('\n') ? '' : '\n'}`);
  }

  frame(frame: Frame) {
    switch (frame.type) {
      case 'host_ready': return this.line(style.green, `✓ host ready · host ${frame.hostPid} · omp ${frame.ompPid}${frame.model ? ` · ${frame.model}` : ''}`);
      case 'session_restore_inspected': return this.line(style.dim, `session restore inspected · ${frame.restored?.resumed ? 'resumed' : 'new session'}`);
      case 'agent_start': return this.line(style.dim, 'responding…');
      case 'message_update': return this.update(frame);
      case 'message_end': return this.endStream();
      case 'host_tool_call': return this.line(style.cyan, `→ ${frame.toolName} ${summarizeArguments(frame.arguments)}`);
      case 'tool_execution_start': return this.line(style.cyan, `→ ${frame.toolName ?? 'tool'} ${summarizeArguments(frame.args)}`);
      case 'tool_execution_end': return this.line(frame.isError ? style.red : style.dim, `${frame.isError ? '✗' : '✓'} ${frame.toolName ?? 'tool'} finished`);
      case 'tool_started': return this.line(style.blue, `▶ ${frame.commandId ?? 'command'} started in container`);
      case 'tool_receipt_reused': return this.line(style.blue, `▶ ${frame.commandId ?? 'command'} reattached to retained container`);
      case 'tool_exited': return this.line(frame.result?.exitCode === 0 ? style.green : style.yellow, `■ command exited ${frame.result?.exitCode ?? 'with unknown status'}`);
      case 'tool_cancelled': return this.line(style.yellow, '■ command cancelled');
      case 'prompt_result': return frame.status === 'error'
        ? this.line(style.red, `✗ turn failed: ${frame.error?.message ?? 'no detail'}`)
        : this.line(style.dim, `turn ${frame.status}`);
      case 'auto_retry_start': return this.line(style.yellow, `retrying provider request${frame.attempt ? ` (attempt ${frame.attempt})` : ''}`);
      case 'auto_compaction_start': return this.line(style.dim, 'compacting context…');
      case 'model_policy_rejected': return this.line(style.red, '✗ blocked an unapproved model or reasoning setting');
      case 'ownership_unresolved': return this.line(style.red, `✗ ownership unresolved: ${frame.reason ?? ''}`);
      case 'process_exit': return this.line(style.yellow, `■ runner exited${frame.code !== undefined && frame.code !== null ? ` (code ${frame.code})` : ''}${frame.signal ? ` (${frame.signal})` : ''}`);
      default: return undefined;
    }
  }

  private update(frame: Frame) {
    const event = frame.assistantMessageEvent ?? {};
    if (event.type === 'text_delta' && typeof event.delta === 'string') {
      if (this.streaming !== 'text') { this.endStream(); this.out(`${style.bold}›${reset} `); this.streaming = 'text'; }
      this.out(event.delta.replace(/\n/g, '\n  '));
    } else if (event.type === 'thinking_start') {
      this.endStream(); this.out(`${style.dim}${style.italic}thinking…`); this.streaming = 'thinking';
    } else if (event.type === 'toolcall_end' && event.toolCall) {
      this.line(style.magenta, `⋯ calling ${event.toolCall.name} ${summarizeArguments(event.toolCall.arguments)}`);
    }
  }
}
