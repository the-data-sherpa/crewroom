import type { Message, Run } from './types';

export function steeringClosedReason(run: Run): string | undefined {
  if (run.status === 'closed') return 'This run is closed.';
  if (run.status === 'completed') return 'This research run is completed.';
  if (run.status === 'cancelled') return 'This run is cancelled.';
  if (run.desiredState === 'cancelled') return 'Cancellation has been requested for this run.';
}

/** An event in message history, which remains true if completed research is reopened. */
export function steeringUndeliveredReason(run: Run): string | undefined {
  if (run.status === 'completed') return 'Research completed before delivery. This message will not be sent if the run reopens.';
  if (run.status === 'closed') return 'The run closed before delivery.';
  if (run.status === 'cancelled' || run.desiredState === 'cancelled') return 'Cancellation was requested before delivery.';
}

/** Preserve the known state of older rows without inventing when it was reached. */
export function steeringTransitions(message: Message): NonNullable<Message['transitions']> {
  if (message.transitions) return message.transitions;
  if (message.delivery === 'pending') return [];
  if (message.delivery === 'not_delivered') return [{ state: 'not_delivered', reason: message.dispositionReason }];
  return [
    { state: 'dispatched' },
    ...(['received', 'applied', 'rejected'].includes(message.delivery) ? [{ state: 'received' as const }] : []),
    ...(['applied', 'rejected'].includes(message.delivery) ? [{ state: message.delivery, reason: message.dispositionReason }] : []),
  ];
}
