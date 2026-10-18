import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join,resolve } from 'node:path';
const source=readFileSync('tests/provider-gateway.test.ts','utf8');
const adapted=source.replace("'../server/runtime/provider-gateway'",JSON.stringify(resolve('scripts/rust/gateway-adapter.ts')))
 .replaceAll("'../server/", "'"+resolve('server')+'/');
const root=process.env.CREWROOM_NATIVE_GATEWAY_RESULTS??'.crewroom/test-results/native-gateway';mkdirSync(root,{recursive:true});
const file=join(root,'gateway.test.ts');writeFileSync(file,adapted);
const result=spawnSync(process.execPath,['--import','tsx','--test',file],{encoding:'utf8',timeout:120000,maxBuffer:8*1024*1024});
process.stdout.write(result.stdout??'');process.stderr.write(result.stderr??'');assert.equal(result.status,0);
const receipt={status:'passed',cases:5,source:'tests/provider-gateway.test.ts',sourceSha256:createHash('sha256').update(source).digest('hex'),adaptedSha256:createHash('sha256').update(adapted).digest('hex'),boundary:'Retained five HTTP scenarios use the Rust provider gateway through a test-only proxy. Synthetic loopback upstreams only. Mutable fixture provider configuration rebuilds the native gateway at the same root before the next request.'};writeFileSync(join(root,'receipt.json'),JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));
