/** Host Docker engine for disposable native fixtures: socket and pinned check image resolved per platform instead of assuming macOS Docker Desktop. */
import {execFileSync} from 'node:child_process';
import {homedir} from 'node:os';
import {join} from 'node:path';

/** Historical macOS (arm64) fixture image; only an image ID, so it cannot be pulled on another host. */
export const legacyCheckImage='sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285';
/** Multi-platform busybox index pinned by digest; provides /bin/sh, cat, head and test for fixture checks. */
export const pinnedCheckReference='busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e';

/** CREWROOM_DOCKER_SOCKET, then a unix:// DOCKER_HOST, then the platform default socket. */
export function dockerSocket(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform,home=homedir()):string{
 if(env.CREWROOM_DOCKER_SOCKET)return env.CREWROOM_DOCKER_SOCKET;
 if(env.DOCKER_HOST){if(!env.DOCKER_HOST.startsWith('unix://'))throw new Error('Native fixtures require a unix:// Docker socket; set CREWROOM_DOCKER_SOCKET.');return env.DOCKER_HOST.slice('unix://'.length);}
 return platform==='darwin'?join(home,'.docker/run/docker.sock'):'/var/run/docker.sock';
}

/** CREWROOM_CHECK_IMAGE, the legacy pin on macOS, otherwise the pinned busybox index pulled locally and returned as its image ID. */
export function checkImage(env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform):string{
 if(env.CREWROOM_CHECK_IMAGE)return env.CREWROOM_CHECK_IMAGE;
 if(platform==='darwin')return legacyCheckImage;
 const docker=(...args:string[])=>execFileSync('docker',args,{encoding:'utf8',env:{...process.env,DOCKER_HOST:`unix://${dockerSocket(env,platform)}`},stdio:['ignore','pipe','pipe']}).trim();
 try{return docker('image','inspect','--format','{{.Id}}',pinnedCheckReference);}
 catch{docker('pull','--quiet',pinnedCheckReference);return docker('image','inspect','--format','{{.Id}}',pinnedCheckReference);}
}
