/** Host Docker engine for disposable native fixtures and verification scripts: one socket resolver and pinned check images instead of assuming macOS Docker Desktop. `scripts/docker_engine.py` mirrors the resolver for Python scripts; `tests/docker-engine.test.ts` keeps the two in step. */
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {readFileSync} from 'node:fs';
import {connect} from 'node:net';
import {homedir} from 'node:os';
import {join} from 'node:path';

/** Historical macOS (arm64) fixture image; only an image ID, so it cannot be pulled on another host. It has /bin/sh and /usr/local/bin/python3. */
export const legacyCheckImage='sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285';
/** Multi-platform busybox index pinned by digest; provides /bin/sh, cat, head and test for fixture checks. */
export const pinnedCheckReference='busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e';
/** Multi-platform python:3.13-alpine index pinned by digest; adds /usr/local/bin/python3 for checks that start detached Python children. */
export const pinnedPythonCheckReference='python@sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a';

export type DockerSocketSource='CREWROOM_DOCKER_SOCKET'|'DOCKER_HOST'|'docker context'|'platform default';
export type DockerSocket={socket:string,source:DockerSocketSource,context?:string};

const remedy='Start the Docker engine, or point Crewroom at a reachable engine with CREWROOM_DOCKER_SOCKET=/path/to/docker.sock (a unix:// DOCKER_HOST or `docker context use <name>` also work).';

function unixSocket(host:string,origin:string):string{
 if(!host.startsWith('unix://')||host.length===7)throw new Error(`${origin} is ${JSON.stringify(host)}, but Crewroom checks need a local unix:// Docker socket. Set CREWROOM_DOCKER_SOCKET=/path/to/docker.sock.`);
 return host.slice('unix://'.length);
}

/** Endpoint of the active Docker CLI context, or undefined for the built-in `default` context. Reads the CLI's own files; never runs `docker`. */
function contextSocket(env:NodeJS.ProcessEnv,home:string):DockerSocket|undefined{
 const config=env.DOCKER_CONFIG||join(home,'.docker');let name=env.DOCKER_CONTEXT;
 if(!name){try{name=JSON.parse(readFileSync(join(config,'config.json'),'utf8')).currentContext;}catch(error:any){if(error?.code!=='ENOENT')throw new Error(`Docker CLI configuration ${join(config,'config.json')} is unreadable: ${error?.message??error}. ${remedy}`);}}
 if(!name||name==='default')return undefined;
 const meta=join(config,'contexts','meta',createHash('sha256').update(name).digest('hex'),'meta.json');let host:unknown;
 try{host=JSON.parse(readFileSync(meta,'utf8'))?.Endpoints?.docker?.Host;}catch(error:any){throw new Error(`Active Docker context ${JSON.stringify(name)} has no readable endpoint at ${meta}: ${error?.message??error}. Run \`docker context use default\` or set CREWROOM_DOCKER_SOCKET.`);}
 if(typeof host!=='string')throw new Error(`Active Docker context ${JSON.stringify(name)} names no Docker endpoint in ${meta}. Run \`docker context use default\` or set CREWROOM_DOCKER_SOCKET.`);
 return {socket:unixSocket(host,`Docker context ${JSON.stringify(name)} endpoint`),source:'docker context',context:name};
}

/** CREWROOM_DOCKER_SOCKET, then a unix:// DOCKER_HOST, then the active Docker CLI context (DOCKER_CONTEXT or `currentContext` in DOCKER_CONFIG/config.json), then the platform default socket. */
export function resolveDockerSocket(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform,home=homedir()):DockerSocket{
 if(env.CREWROOM_DOCKER_SOCKET)return {socket:env.CREWROOM_DOCKER_SOCKET,source:'CREWROOM_DOCKER_SOCKET'};
 if(env.DOCKER_HOST)return {socket:unixSocket(env.DOCKER_HOST,'DOCKER_HOST'),source:'DOCKER_HOST'};
 return contextSocket(env,home)??{socket:platform==='darwin'?join(home,'.docker/run/docker.sock'):'/var/run/docker.sock',source:'platform default'};
}

export function dockerSocket(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform,home=homedir()):string{return resolveDockerSocket(env,platform,home).socket;}

/** Message for an engine that did not answer `/_ping` on the resolved socket. */
export function unreachableEngineMessage(resolved:DockerSocket,reason:string):string{
 const from=resolved.source==='docker context'?`Docker context ${JSON.stringify(resolved.context)}`:resolved.source;
 const access=/EACCES|permission denied/i.test(reason)?' Add this user to the socket\'s group (for example `docker`) or use a rootless engine socket.':'';
 return `Docker engine is not reachable at ${resolved.socket} (from ${from}): ${reason}.${access} ${remedy}`;
}

/** Resolves the socket and confirms the engine answers `GET /_ping`; otherwise rejects naming the socket tried and the remedy. */
export async function requireDockerEngine(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform,home=homedir(),timeoutMs=5000):Promise<{socket:string}>{
 const resolved=resolveDockerSocket(env,platform,home);
 const reason=await new Promise<string|undefined>(resolve=>{
  let reply='';const conn=connect(resolved.socket);const timer=setTimeout(()=>{conn.destroy();resolve(`no reply within ${timeoutMs} ms`);},timeoutMs);
  const done=(value:string|undefined)=>{clearTimeout(timer);conn.destroy();resolve(value);};
  conn.on('connect',()=>conn.write('GET /_ping HTTP/1.0\r\nHost: docker\r\n\r\n'));
  conn.on('data',chunk=>{reply+=chunk;});
  conn.on('end',()=>done(/^HTTP\/1\.[01] 200\b/.test(reply)&&reply.trimEnd().endsWith('OK')?undefined:`unexpected /_ping reply ${JSON.stringify(reply.split('\r\n')[0]??'')}`));
  conn.on('error',(error:Error)=>done(error.message));
 });
 if(reason)throw new Error(unreachableEngineMessage(resolved,reason));
 return {socket:resolved.socket};
}

function localImage(reference:string,env:NodeJS.ProcessEnv,platform:NodeJS.Platform):string{
 const docker=(...args:string[])=>execFileSync('docker',args,{encoding:'utf8',env:{...process.env,DOCKER_HOST:`unix://${dockerSocket(env,platform)}`,DOCKER_CONTEXT:''},stdio:['ignore','pipe','pipe']}).trim();
 try{return docker('image','inspect','--format','{{.Id}}',reference);}
 catch{docker('pull','--quiet',reference);return docker('image','inspect','--format','{{.Id}}',reference);}
}

/** CREWROOM_CHECK_IMAGE, the legacy pin on macOS, otherwise the pinned busybox index pulled locally and returned as its image ID. */
export function checkImage(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform):string{
 if(env.CREWROOM_CHECK_IMAGE)return env.CREWROOM_CHECK_IMAGE;
 if(platform==='darwin')return legacyCheckImage;
 return localImage(pinnedCheckReference,env,platform);
}

/** Check image that also provides /usr/local/bin/python3: CREWROOM_PYTHON_CHECK_IMAGE, the legacy pin on macOS, otherwise the pinned python index pulled locally and returned as its image ID. */
export function pythonCheckImage(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform):string{
 if(env.CREWROOM_PYTHON_CHECK_IMAGE)return env.CREWROOM_PYTHON_CHECK_IMAGE;
 if(platform==='darwin')return legacyCheckImage;
 return localImage(pinnedPythonCheckReference,env,platform);
}
