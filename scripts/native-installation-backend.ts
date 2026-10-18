/** Production daemon launcher for disposable installation/restore evidence. */
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
export async function nativeInstallationBackend(config:any, options:{existingRuntime?:boolean}={}) {
 const runtime=join(config.root,'runtime-config.json');
 mkdirSync(config.root,{recursive:true,mode:0o700});
 if(!options.existingRuntime)writeFileSync(runtime,JSON.stringify(config.runtime),{mode:0o600});
 else assert.ok(existsSync(runtime),'Restored private runtime configuration is missing.');
 const binary=join(resolve(config.binaryDirectory??'target/debug'),'crewroomd');
 const proc=spawn(binary,['--data-dir',config.root,'--roles',config.rolePath,'--port',String(config.serverPort)],{stdio:['ignore','pipe','pipe'],env:{...process.env,CREWROOM_RUNTIME_CONFIG:runtime,CREWROOM_TMUX:config.tmuxBinary??'/opt/homebrew/bin/tmux'}});
 let stderr='';proc.stderr.on('data',b=>stderr+=b);proc.stdout.resume();const exited=once(proc,'exit');
 async function close() {
  if(proc.exitCode===null&&proc.signalCode===null) {
   proc.kill('SIGTERM');const abort=new AbortController();
   const stopped=await Promise.race([exited.then(()=>true),sleep(60000,false,{signal:abort.signal}).catch(()=>true)]);abort.abort();
   if(!stopped){proc.kill('SIGKILL');await exited;throw new Error('Native installation daemon required forced cleanup: '+stderr);}
  }
  await exited;assert.equal(proc.exitCode,0,'Native daemon shutdown failed: '+stderr);
 }
 try {
  const deadline=Date.now()+30000;let info:any;
  while(Date.now()<deadline) {
   assert.ok(proc.exitCode===null&&proc.signalCode===null,'Native daemon exited before readiness: '+stderr);
   const path=join(config.root,'native-connection.json');
   if(existsSync(path)) {
    try {const candidate=JSON.parse(readFileSync(path,'utf8'));if(candidate.pid===proc.pid){const r=await fetch(candidate.origin+'/api/health',{headers:{Authorization:`Bearer ${candidate.token}`},signal:AbortSignal.timeout(1000)});if(r.ok){info=candidate;break;}}} catch { /* Published descriptor can precede listener readiness. */ }
   }
   await sleep(25);
  }
  assert.ok(info,'Native daemon did not become ready: '+stderr);
  return {pid:proc.pid!,origin:String(info.origin),token:String(info.token),close};
 } catch(error) {await close().catch(()=>{});throw error;}
}
