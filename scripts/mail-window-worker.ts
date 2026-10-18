/** A disposable process killed at one receive boundary; the parent verifies recovery. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { Messaging, RustMailClient } from '../server/runtime/messaging';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const [messageId, recipientAttemptId, window] = process.argv.slice(3);
const store = new Workbench(join(config.root, 'workflow.sqlite'));
const messages = new Messaging(store, new RustMailClient(config.url, config.token), config.project, 86400);
if (window !== 'before-persistence') store.acceptMessage(messageId);
if (window === 'before-ack' || window === 'after-ack') {
  const response = await fetch(config.controlUrl + `/drop-ack-${window === 'before-ack' ? 'before' : 'after'}`, { method: 'POST' });
  assert.ok(response.ok); await assert.rejects(messages.accept(messageId));
}
if (window === 'after-processing') {
  await messages.accept(messageId); store.recordDispatch(messageId, true);
  store.prepareMessage(recipientAttemptId, `effect-${messageId}`, { to: 'Coordinator', body: `Effect for ${messageId}`, replyTo: messageId });
}
// Deliberately skip graceful store close. SQLite must retain committed acceptance/effects.
process.kill(process.pid, 'SIGKILL');
