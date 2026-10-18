import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const out=process.env.CREWROOM_NATIVE_CONTAINER_RESULTS??'.crewroom/test-results/native-containers';mkdirSync(out,{recursive:true});
const receipts=[];
for(const sourcePath of ['tests/container.integration.ts','tests/cleanup.integration.ts']){
 const source=readFileSync(sourcePath,'utf8');let adapted=source;
 if(sourcePath.includes('cleanup'))adapted=adapted.slice(0,adapted.indexOf("test('SEC-05: startup"));
 adapted=adapted.replaceAll("'../server/runtime/container-command'",JSON.stringify(resolve('scripts/rust/container-adapter.ts'))).replaceAll("'./server/runtime/container-command.ts'",JSON.stringify(resolve('scripts/rust/container-adapter.ts')))
 .replaceAll("'../server/runtime/processes'",JSON.stringify(resolve('scripts/rust/process-adapter.ts'))).replaceAll("'./server/runtime/processes.ts'",JSON.stringify(resolve('scripts/rust/process-adapter.ts')));
 const file=join(out,sourcePath.split('/').at(-1)!);writeFileSync(file,adapted);
 const r=spawnSync(process.execPath,['--import','tsx','--test',file],{encoding:'utf8',timeout:180000,maxBuffer:8*1024*1024});process.stdout.write(r.stdout??'');process.stderr.write(r.stderr??'');
 receipts.push({source:sourcePath,sourceSha256:createHash('sha256').update(source).digest('hex'),adaptedSha256:createHash('sha256').update(adapted).digest('hex'),exitCode:r.status});
 writeFileSync(join(out,'receipt.json'),JSON.stringify({status:receipts.every(r=>r.exitCode===0)?'passed':'failed',cases:5,receipts,boundary:'Four retained real Docker cases and the independent cleanup-controller scenario use Rust. The later supervisor startup-cleanup case remains an N3.3 integration gate.'},null,2)+'\n');assert.equal(r.status,0);
}
