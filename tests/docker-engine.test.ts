import assert from 'node:assert/strict';
import { execFile, execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { checkImage, dockerSocket, legacyCheckImage, pythonCheckImage, requireDockerEngine, resolveDockerSocket } from '../scripts/rust/docker-engine';

/** A synthetic Docker CLI configuration directory with named contexts and an optional current one. */
function dockerConfig(t: TestContext, contexts: Record<string, unknown>, current?: string) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-docker-config-')); t.after(() => rmSync(root, { recursive: true, force: true }));
  const config = join(root, '.docker');
  for (const [name, meta] of Object.entries(contexts)) {
    const dir = join(config, 'contexts', 'meta', createHash('sha256').update(name).digest('hex')); mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, 'meta.json'), typeof meta === 'string' ? meta : JSON.stringify(meta));
  }
  mkdirSync(config, { recursive: true });
  if (current !== undefined) writeFileSync(join(config, 'config.json'), JSON.stringify({ auths: {}, currentContext: current }));
  return { home: root, config };
}
const endpoint = (host: string) => ({ Name: 'synthetic', Metadata: {}, Endpoints: { docker: { Host: host, SkipTLSVerify: false } } });

test('Docker socket resolution order: Crewroom override, DOCKER_HOST, active Docker context, platform default', t => {
  const { home, config } = dockerConfig(t, { rootless: endpoint('unix:///run/user/1000/docker.sock'), other: endpoint('unix:///run/other.sock') }, 'rootless');
  const all = { CREWROOM_DOCKER_SOCKET: '/run/a.sock', DOCKER_HOST: 'unix:///run/b.sock', DOCKER_CONTEXT: 'other' };
  assert.deepEqual(resolveDockerSocket(all, 'linux', home), { socket: '/run/a.sock', source: 'CREWROOM_DOCKER_SOCKET' });
  assert.deepEqual(resolveDockerSocket({ ...all, CREWROOM_DOCKER_SOCKET: '' }, 'darwin', home), { socket: '/run/b.sock', source: 'DOCKER_HOST' });
  assert.deepEqual(resolveDockerSocket({ DOCKER_CONTEXT: 'other' }, 'linux', home), { socket: '/run/other.sock', source: 'docker context', context: 'other' });
  assert.deepEqual(resolveDockerSocket({}, 'linux', home), { socket: '/run/user/1000/docker.sock', source: 'docker context', context: 'rootless' });
  assert.deepEqual(resolveDockerSocket({ DOCKER_CONFIG: config }, 'linux', '/nonexistent-home'), { socket: '/run/user/1000/docker.sock', source: 'docker context', context: 'rootless' });
  assert.deepEqual(resolveDockerSocket({ DOCKER_CONTEXT: 'default' }, 'linux', home), { socket: '/var/run/docker.sock', source: 'platform default' });
  assert.equal(dockerSocket({}, 'linux', '/nonexistent-home'), '/var/run/docker.sock');
  assert.equal(dockerSocket({}, 'darwin', '/home/u'), '/home/u/.docker/run/docker.sock');
  const defaulted = dockerConfig(t, {}, 'default');
  assert.equal(dockerSocket({}, 'darwin', defaulted.home), join(defaulted.home, '.docker/run/docker.sock'));
});

test('Docker socket resolution refuses remote or unreadable engines with a remedy', t => {
  const { home } = dockerConfig(t, { remote: endpoint('tcp://10.0.0.2:2376'), broken: '{', empty: { Endpoints: {} } }, 'missing');
  assert.throws(() => dockerSocket({ DOCKER_HOST: 'tcp://127.0.0.1:2375' }, 'linux', home), /DOCKER_HOST.*unix:\/\/.*CREWROOM_DOCKER_SOCKET/);
  assert.throws(() => dockerSocket({ DOCKER_CONTEXT: 'remote' }, 'linux', home), /Docker context "remote" endpoint.*unix:\/\//);
  assert.throws(() => dockerSocket({ DOCKER_CONTEXT: 'broken' }, 'linux', home), /context "broken".*docker context use default/);
  assert.throws(() => dockerSocket({ DOCKER_CONTEXT: 'empty' }, 'linux', home), /context "empty" names no Docker endpoint/);
  assert.throws(() => dockerSocket({}, 'linux', home), /context "missing" has no readable endpoint/);
});

test('Python verification scripts resolve the same Docker socket as the TypeScript fixtures', t => {
  const { home, config } = dockerConfig(t, { rootless: endpoint('unix:///run/user/1000/docker.sock'), remote: endpoint('tcp://10.0.0.2:2376') }, 'rootless');
  const python = (env: Record<string, string>, platform: string) => {
    const code = 'import json,sys;sys.path.insert(0,"scripts");import docker_engine as d;env=json.loads(sys.argv[1])\ntry:print(json.dumps(d.resolve_docker_socket(env,sys.argv[2],sys.argv[3])))\nexcept d.DockerEngineError as e:print(json.dumps({"error":str(e)}))';
    return JSON.parse(execFileSync('python3', ['-I', '-c', code, JSON.stringify(env), platform, home], { encoding: 'utf8', cwd: process.cwd() }));
  };
  const cases: [Record<string, string>, NodeJS.Platform][] = [
    [{ CREWROOM_DOCKER_SOCKET: '/run/a.sock', DOCKER_HOST: 'unix:///run/b.sock' }, 'linux'], [{ DOCKER_HOST: 'unix:///run/b.sock' }, 'linux'],
    [{}, 'linux'], [{ DOCKER_CONTEXT: 'default' }, 'darwin'], [{ DOCKER_CONFIG: config }, 'linux'], [{ DOCKER_CONTEXT: 'remote' }, 'linux'], [{ DOCKER_HOST: 'tcp://h:1' }, 'linux'],
  ];
  for (const [env, platform] of cases) {
    let expected: unknown; try { expected = resolveDockerSocket(env, platform, home); } catch (error) { expected = { error: (error as Error).message }; }
    assert.deepEqual(python(env, platform), expected, JSON.stringify(env));
  }
});

test('an unreachable Docker engine fails naming the socket tried and the remedy', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-engine-')); t.after(() => rmSync(root, { recursive: true, force: true }));
  const missing = join(root, 'missing.sock');
  await assert.rejects(requireDockerEngine({ CREWROOM_DOCKER_SOCKET: missing }, 'linux', root),
    (error: Error) => error.message.includes(`not reachable at ${missing} (from CREWROOM_DOCKER_SOCKET)`) && /ENOENT/.test(error.message) && /Start the Docker engine.*CREWROOM_DOCKER_SOCKET=/.test(error.message));
  const script = spawnSync('python3', ['-I', 'scripts/docker_engine.py', '--require'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: root, CREWROOM_DOCKER_SOCKET: missing } });
  assert.equal(script.status, 2); assert.match(script.stderr, new RegExp(`not reachable at ${missing} \\(from CREWROOM_DOCKER_SOCKET\\): ENOENT.*Start the Docker engine`));
  const replies = { ok: 'HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK', broken: 'HTTP/1.1 500 Internal Server Error\r\n\r\nnope' };
  for (const [name, reply] of Object.entries(replies)) {
    const path = join(root, `${name}.sock`); const server = createServer(conn => conn.once('data', () => conn.end(reply)));
    await new Promise<void>(resolve => server.listen(path, resolve)); t.after(() => server.close());
    const engine = { DOCKER_HOST: `unix://${path}` };
    if (name === 'ok') assert.deepEqual(await requireDockerEngine(engine, 'linux', root), { socket: path });
    else await assert.rejects(requireDockerEngine(engine, 'linux', root), new RegExp(`not reachable at ${path} \\(from DOCKER_HOST\\): unexpected /_ping reply "HTTP/1.1 500`));
    // Asynchronous, so this process can keep serving the fixture socket while Python pings it.
    const checked = await new Promise<{ status: number | null, stdout: string, stderr: string }>(resolve => execFile('python3', ['-I', 'scripts/docker_engine.py', '--require'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: root, ...engine } }, (error, stdout, stderr) => resolve({ status: error ? (error.code as number) : 0, stdout, stderr })));
    if (name === 'ok') assert.deepEqual(JSON.parse(checked.stdout), { socket: path, source: 'DOCKER_HOST' });
    else { assert.equal(checked.status, 2); assert.match(checked.stderr, /unexpected \/_ping reply "HTTP\/1.1 500/); }
  }
});

test('native fixture check images honour explicit pins and keep the historical macOS image', () => {
  const pinned = 'sha256:' + 'a'.repeat(64), python = 'sha256:' + 'b'.repeat(64);
  assert.equal(checkImage({ CREWROOM_CHECK_IMAGE: pinned }, 'linux'), pinned);
  assert.equal(checkImage({}, 'darwin'), legacyCheckImage);
  assert.equal(pythonCheckImage({ CREWROOM_PYTHON_CHECK_IMAGE: python, CREWROOM_CHECK_IMAGE: pinned }, 'linux'), python);
  assert.equal(pythonCheckImage({}, 'darwin'), legacyCheckImage);
});
