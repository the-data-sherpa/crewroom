// Test boundary: forwards production preflight RPC requests to the real guarded OMP fixture.
import { createInterface } from 'node:readline';
import { resolveAssignment, configureAssignment, isolatedModelSettings } from '../server/runtime/model-policy';

const lines = createInterface({ input: process.stdin });
let first: (value: any) => void;
const input = new Promise<any>(resolve => { first = resolve; });
const pending = new Map<number, (value: any) => void>();
let serial = 0;
lines.on('line', line => {
  const value = JSON.parse(line);
  if (value.replyTo) pending.get(value.replyTo)?.(value);
  else first(value);
});
const emit = (value: unknown) => process.stdout.write(JSON.stringify(value) + '\n');
try {
  const value = await input;
  const plan = resolveAssignment(value.role, value.catalog, value.policy ?? {}, value.requirements ?? {});
  if (value.operation === 'settings') emit({ done: true, result: isolatedModelSettings(plan) });
  else {
    const identity = await configureAssignment({ async command(type, args = {}) {
      const id = ++serial;
      const response = new Promise<any>(resolve => pending.set(id, resolve));
      emit({ rpc: true, id, type, args });
      const reply = await response; pending.delete(id);
      if (reply.error) throw new Error(reply.error);
      return reply.result;
    } }, plan);
    emit({ done: true, result: { plan, identity } });
  }
} catch (error) { emit({ done: true, error: error instanceof Error ? error.message : String(error) }); }
finally { lines.close(); process.stdin.pause(); }
