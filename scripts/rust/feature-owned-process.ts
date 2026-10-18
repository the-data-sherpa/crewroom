/** Optional receipt-owned process boundary for feature verification fixtures. */
import {spawn, type ChildProcessWithoutNullStreams} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {join} from 'node:path';
import {launchOwned} from './process-adapter';

export function spawnFixtureProcess(
  role: 'daemon' | 'native-ui', command: string, args: string[], env: NodeJS.ProcessEnv,
): ChildProcessWithoutNullStreams {
  const resources = process.env.CREWROOM_FEATURE_RESOURCE_ROOT;
  if (!resources) return spawn(command, args, {env, stdio:'pipe'});
  // Owned specs are retained on disk. Carry only the fixture's runtime inputs,
  // never the caller's provider credentials or unrelated login environment.
  const keys = ['PATH', 'HOME', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TERM', 'RUST_BACKTRACE',
    'CREWROOM_TEST_RUN', 'CREWROOM_TEST_REPO',
    ...(role === 'daemon' ? ['CREWROOM_RUNTIME_CONFIG', 'CREWROOM_TMUX'] : ['CREWROOM_NATIVE_JOURNEY_PLAN'])];
  const fixtureEnv = Object.fromEntries(keys.filter(key => env[key] !== undefined).map(key => [key, env[key]]));
  // launchOwned persists its specification before exec; its checked identity is
  // recoverable even if the Node fixture dies before collecting an exit event.
  return launchOwned(join(resources, `${role}-${randomUUID()}`), command, args,
    {cwd:process.cwd(), env:fixtureEnv});
}
