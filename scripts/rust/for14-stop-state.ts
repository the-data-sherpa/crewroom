/** Acceptance cleanup needs both settled comparison records and released slot. */
export function comparisonSettled(comparison: any, queue: any): boolean {
  if (!comparison || !['completed', 'incomplete', 'cancelled'].includes(comparison.state)
    || !Array.isArray(comparison.cells) || !queue || !Array.isArray(queue.plans) || queue.slot != null) return false;
  return comparison.cells.every((cell: any) => {
    if (!cell.runId) return cell.state === 'cancelled';
    return ['completed', 'incomplete', 'unavailable', 'cancelled'].includes(cell.result?.state)
      && ['completed', 'cancelled'].includes(cell.runState)
      && cell.result?.cleanup?.state === 'stopped'
      && Array.isArray(cell.attempts)
      && cell.attempts.every((attempt: any) => attempt.state === 'admitted' || attempt.ownership === 'stopped');
  });
}
