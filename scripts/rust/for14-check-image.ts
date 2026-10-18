/** Build and verify immutable required-check bytes; this performs no inference. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { digest, fixtureRoot, readCasePack } from './for14-case-pack';

export function buildCheckImage(destination: string) {
  assert.ok(Number(process.versions.node.split('.')[0]) >= 24, 'Use the repository Node 24+ runtime');
  const root = resolve(destination), context = join(root, 'context'), pack = readCasePack();
  mkdirSync(context, { recursive: true, mode: 0o700 });
  const require = createRequire(import.meta.url);
  const playwright = resolve(require.resolve('playwright-core/package.json'), '..');
  cpSync(playwright, join(context, 'playwright-core'), { recursive: true, dereference: true });
  cpSync(join(fixtureRoot, 'checks'), join(context, 'checks'), { recursive: true });
  cpSync(join(fixtureRoot, 'Dockerfile'), join(context, 'Dockerfile'));
  const imageFile = join(root, 'image-id.txt');
  execFileSync('docker', ['build', '--quiet', '--iidfile', imageFile, '--label', 'crewroom.fixture=for14',
    '--tag', 'crewroom-for14-checks:' + pack.packHash.slice(0, 16), context], { stdio: 'inherit', timeout: 600000 });
  const image = readFileSync(imageFile, 'utf8').trim();
  assert.match(image, /^sha256:[a-f0-9]{64}$/);
  const output = execFileSync('docker', ['run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL',
    '--security-opt', 'no-new-privileges', '--entrypoint', '/usr/bin/sha256sum', image,
    ...pack.checks.map(check => '/opt/crewroom-checks/' + check.path)], { encoding: 'utf8', timeout: 30000 });
  const observed = output.trim().split('\n').map(line => {
    const match = /^([a-f0-9]{64})\s+\/opt\/crewroom-checks\/(.+)$/.exec(line);
    assert.ok(match, 'Unexpected check hash observation');
    return { path: match[2], sha256: match[1] };
  });
  assert.deepEqual(observed, pack.checks.map(({ path, sha256 }) => ({ path, sha256 })));
  const receipt = { version: 1, mode: 'deterministic-image-preflight', image,
    packHash: pack.packHash, dockerfileHash: digest(readFileSync(join(context, 'Dockerfile'))),
    playwrightVersion: JSON.parse(readFileSync(join(playwright, 'package.json'), 'utf8')).version,
    checks: observed, networklessVerification: true, verifiedAt: new Date().toISOString() };
  writeFileSync(join(root, 'image-receipt.json'), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
  return receipt;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 3) throw new Error('Usage: tsx scripts/rust/for14-check-image.ts <retained-output-directory>');
  console.log(JSON.stringify(buildCheckImage(process.argv[2]), null, 2));
}
