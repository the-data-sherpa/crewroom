import assert from 'node:assert/strict';
import { test } from 'node:test';
import { checkImage, dockerSocket, legacyCheckImage } from '../scripts/rust/docker-engine';

test('native fixtures resolve the Docker socket from explicit configuration before the platform default', () => {
  assert.equal(dockerSocket({ CREWROOM_DOCKER_SOCKET: '/run/a.sock', DOCKER_HOST: 'unix:///run/b.sock' }, 'linux', '/home/u'), '/run/a.sock');
  assert.equal(dockerSocket({ DOCKER_HOST: 'unix:///run/b.sock' }, 'darwin', '/home/u'), '/run/b.sock');
  assert.equal(dockerSocket({}, 'linux', '/home/u'), '/var/run/docker.sock');
  assert.equal(dockerSocket({}, 'darwin', '/home/u'), '/home/u/.docker/run/docker.sock');
  assert.throws(() => dockerSocket({ DOCKER_HOST: 'tcp://127.0.0.1:2375' }, 'linux', '/home/u'), /unix:\/\//);
});

test('native fixture check image honours an explicit pin and keeps the historical macOS image', () => {
  const pinned = 'sha256:' + 'a'.repeat(64);
  assert.equal(checkImage({ CREWROOM_CHECK_IMAGE: pinned }, 'linux'), pinned);
  assert.equal(checkImage({}, 'darwin'), legacyCheckImage);
});
