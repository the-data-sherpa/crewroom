import { rmSync } from 'node:fs';
export default function cleanup() {
  if (process.env.CREWROOM_E2E_DATA) rmSync(process.env.CREWROOM_E2E_DATA, { recursive: true, force: true });
}
