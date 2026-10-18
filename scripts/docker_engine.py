#!/usr/bin/env python3
"""Docker socket resolver for Python verification scripts.

Mirrors `scripts/rust/docker-engine.ts`; `tests/docker-engine.test.ts` runs both
against the same environments. Order: CREWROOM_DOCKER_SOCKET, a unix://
DOCKER_HOST, the active Docker CLI context (DOCKER_CONTEXT, else
`currentContext` in DOCKER_CONFIG/config.json), then the platform default.
`python3 scripts/docker_engine.py` prints the resolution as JSON;
`--require` also checks that the engine answers `/_ping`.
"""
import errno
import hashlib
import json
import os
import socket as sockets
import sys
from pathlib import Path

REMEDY = ('Start the Docker engine, or point Crewroom at a reachable engine with '
          'CREWROOM_DOCKER_SOCKET=/path/to/docker.sock (a unix:// DOCKER_HOST or '
          '`docker context use <name>` also work).')


class DockerEngineError(RuntimeError):
    pass


def _unix(host, origin):
    if not host.startswith('unix://') or host == 'unix://':
        raise DockerEngineError(f'{origin} is {json.dumps(host)}, but Crewroom checks need a local unix:// '
                                'Docker socket. Set CREWROOM_DOCKER_SOCKET=/path/to/docker.sock.')
    return host[len('unix://'):]


def _context(env, home):
    config = Path(env.get('DOCKER_CONFIG') or Path(home) / '.docker')
    name = env.get('DOCKER_CONTEXT')
    if not name:
        try:
            name = json.loads((config / 'config.json').read_text()).get('currentContext')
        except FileNotFoundError:
            name = None
        except (OSError, ValueError, AttributeError) as error:
            raise DockerEngineError(f'Docker CLI configuration {config / "config.json"} is unreadable: {error}. {REMEDY}')
    if not name or name == 'default':
        return None
    meta = config / 'contexts' / 'meta' / hashlib.sha256(name.encode()).hexdigest() / 'meta.json'
    try:
        host = json.loads(meta.read_text()).get('Endpoints', {}).get('docker', {}).get('Host')
    except (OSError, ValueError, AttributeError) as error:
        raise DockerEngineError(f'Active Docker context {json.dumps(name)} has no readable endpoint at {meta}: {error}. '
                                'Run `docker context use default` or set CREWROOM_DOCKER_SOCKET.')
    if not isinstance(host, str):
        raise DockerEngineError(f'Active Docker context {json.dumps(name)} names no Docker endpoint in {meta}. '
                                'Run `docker context use default` or set CREWROOM_DOCKER_SOCKET.')
    return {'socket': _unix(host, f'Docker context {json.dumps(name)} endpoint'), 'source': 'docker context', 'context': name}


def resolve_docker_socket(env=None, platform=None, home=None):
    env = os.environ if env is None else env
    platform = sys.platform if platform is None else platform
    home = str(Path.home()) if home is None else home
    if env.get('CREWROOM_DOCKER_SOCKET'):
        return {'socket': env['CREWROOM_DOCKER_SOCKET'], 'source': 'CREWROOM_DOCKER_SOCKET'}
    if env.get('DOCKER_HOST'):
        return {'socket': _unix(env['DOCKER_HOST'], 'DOCKER_HOST'), 'source': 'DOCKER_HOST'}
    found = _context(env, home)
    if found:
        return found
    default = str(Path(home) / '.docker/run/docker.sock') if platform == 'darwin' else '/var/run/docker.sock'
    return {'socket': default, 'source': 'platform default'}


def docker_socket(env=None, platform=None, home=None):
    return resolve_docker_socket(env, platform, home)['socket']


def unreachable_engine_message(resolved, reason):
    origin = f'Docker context {json.dumps(resolved["context"])}' if resolved['source'] == 'docker context' else resolved['source']
    access = (" Add this user to the socket's group (for example `docker`) or use a rootless engine socket."
              if 'EACCES' in reason or 'ermission denied' in reason else '')
    return f'Docker engine is not reachable at {resolved["socket"]} (from {origin}): {reason}.{access} {REMEDY}'


def require_docker_engine(env=None, platform=None, home=None, timeout=5.0):
    """Return the resolved socket after the engine answers GET /_ping; raise DockerEngineError naming it otherwise."""
    resolved = resolve_docker_socket(env, platform, home)
    try:
        with sockets.socket(sockets.AF_UNIX, sockets.SOCK_STREAM) as conn:
            conn.settimeout(timeout)
            conn.connect(resolved['socket'])
            conn.sendall(b'GET /_ping HTTP/1.0\r\nHost: docker\r\n\r\n')
            reply = b''
            while chunk := conn.recv(4096):
                reply += chunk
    except OSError as error:
        code = error.__class__.__name__ if error.errno is None else errno.errorcode.get(error.errno, str(error.errno))
        raise DockerEngineError(unreachable_engine_message(resolved, f'{code} {error.strerror or error}')) from None
    text = reply.decode(errors='replace')
    if not (text.startswith(('HTTP/1.0 200', 'HTTP/1.1 200')) and text.rstrip().endswith('OK')):
        raise DockerEngineError(unreachable_engine_message(resolved, f'unexpected /_ping reply {json.dumps(text.splitlines()[0] if text else "")}'))
    return resolved['socket']


if __name__ == '__main__':
    try:
        if '--require' in sys.argv[1:]:
            require_docker_engine()
        print(json.dumps(resolve_docker_socket()))
    except DockerEngineError as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
