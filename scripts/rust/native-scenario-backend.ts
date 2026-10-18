/** Reuses the production native service; only provider endpoints/credentials are injected by its existing example. */
import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync,readFileSync} from 'node:fs';
import {join} from 'node:path';
import {defaultRoles} from '../../server/roles';
import {NativeDaemon} from './daemon-client';
export async function nativeScenarioBackend(config:any,publication?:any[]){
 mkdirSync(config.root,{recursive:true,mode:0o700});
 const roles=defaultRoles.map(role=>{const selected=config.selectedRoles[role.id];return selected?{...role,model:selected.selector,effort:selected.effort}:role;});
 writeFileSync(config.rolePath,JSON.stringify(roles),{mode:0o600});
 const daemon=new NativeDaemon({authority:config.root,roles:config.rolePath,runtime:config.runtime,nativePort:config.serverPort,providerFixtureBinary:config.providerFixtureBinary,environment:{PATH:process.env.PATH,HOME:process.env.HOME},...(publication?{publicationRemote:{connections:publication.map(({credentials:_,...connection})=>connection)}}:{})});
 await daemon.ready;
 writeFileSync(join(config.root,'scenario-daemon.json'),JSON.stringify({pid:daemon.proc.pid,binary:daemon.proc.spawnfile,providerInjection:!!publication,descriptor:join(config.root,'native-connection.json')}));
 return {origin:String(daemon.info.origin),token:String(daemon.info.token),async close(){await daemon.close();assert.equal(daemon.proc.exitCode,0,daemon.stderr);}};
}
export async function nativeIdentities(config:any,state:any){
 const identities=[];
 for(const assignment of state.assignments){
  const attempt=state.attempts.find((a:any)=>a.id===assignment.currentAttemptId),selected=config.selectedRoles[assignment.role.id]??config.selectedRoles.triage;
  const root=join(config.runtime.root,attempt.id),host=JSON.parse(readFileSync(join(root,'host.json'),'utf8')),intent=JSON.parse(readFileSync(join(root,'host-intent.json'),'utf8'));
  assert.equal(intent.spec.command,join(config.binaryDirectory,'crewroom-host'));assert.equal(host.helper,join(config.binaryDirectory,'crewroom-process'));
  // Publication may stop the team as soon as readiness is established. Retain
  // the actual OMP identity observed at host startup instead of probing an
  // endpoint after its correctly owned process has stopped. The outer harness
  // independently checks every actual provider request's agent/model identity.
  const observation=state.events.findLast((e:any)=>e.attemptId===attempt.id&&e.event.type==='host_ready');assert.ok(observation,'Actual OMP host_ready identity must be retained');const observed=observation.event;
  assert.equal(observed.model.provider,selected.provider);assert.equal(observed.model.id,selected.id);assert.equal(observed.model.api,selected.api);assert.equal(observed.thinkingLevel,selected.effort);assert.equal(observed.hostPid,attempt.hostPid);assert.equal(observed.ompPid,attempt.ompPid);assert.equal(attempt.model.resolved.selector,selected.selector);
  identities.push({name:assignment.name,assignmentId:assignment.id,attemptId:attempt.id,hostPid:attempt.hostPid,ompPid:attempt.ompPid,sessionId:attempt.sessionId,planned:attempt.model,observed,observation:'persisted actual OMP host_ready',eventCursor:observation.cursor});
 }
 assert.equal(new Set(identities.flatMap(a=>[a.hostPid,a.ompPid])).size,identities.length*2);
 return identities;
}
