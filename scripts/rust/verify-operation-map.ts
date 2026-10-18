/** Close the public Workbench inventory using actual successful oracle dispatches.
 * A dispatched method is bounded transaction evidence, never runner/provider proof. */
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
const source=readFileSync('server/workbench.ts','utf8');
const workbench=source.slice(source.indexOf('export class Workbench'));
const methods=[...workbench.matchAll(/^  (\w+)\(/gm)].map(m=>m[1]);
assert.equal(methods.length,154);assert.equal(new Set(methods).size,154);
const declared=readFileSync('docs/native-workbench-operation-map.md','utf8').split('\n').filter(line=>line.startsWith('| N')).flatMap(line=>[...line.split('|')[2].matchAll(/`(\w+)`/g)].map(m=>m[1]));
assert.deepEqual([...declared].sort(),[...methods].sort(),'Every retained public method has a declared completion boundary');
const trace=process.env.CREWROOM_ORACLE_COVERAGE;assert.ok(trace,'Run after compatibility oracles with an explicit disposable coverage trace');
const receipts=readFileSync(trace,'utf8').trim().split('\n').map(line=>JSON.parse(line));
const counts=new Map<string,{passed:number;refused:number}>();
for(const row of receipts){const count=counts.get(row.method)??{passed:0,refused:0};if(row.ok)count.passed++;else count.refused++;counts.set(row.method,count);}
const directory='crates/crewroom-core/src';const files=readdirSync(directory).filter(f=>f.endsWith('.rs')).map(f=>({path:join(directory,f),source:readFileSync(join(directory,f),'utf8')}));
const aliases:Record<string,string>={deferDependencyLaunch:'defer_unadmitted_launch',queue:'queue_with_roles'};
const rows=methods.map(method=>{
 if(method==='constructor')return {method,status:'partial',native:'Store::open',scope:'Fresh/schema-26 opens exercised by every native oracle process; historical schema upgrades remain N7.1.'};
 if(method==='close')return {method,status:'covered',native:'Store owns rusqlite::Connection; Drop closes it',scope:'Each native operation exits and the next reopens the same disposable database.'};
 if(method==='onChange')return {method,status:'pending',native:'AppState watch channel covers HTTP/WS only',scope:'Controller event wiring remains N3.3; execution stays unavailable.'};
 const name=aliases[method]??method.replace(/[A-Z]/g,c=>'_'+c.toLowerCase());
 const definition=files.find(f=>new RegExp('\\bfn '+name+'\\b').test(f.source));assert.ok(definition,method+' native entry point');
 const count=counts.get(method);assert.ok(count && count.passed>0,method+' needs an actual successful oracle dispatch');
 const line=definition.source.slice(0,definition.source.search(new RegExp('\\bfn '+name+'\\b'))).split('\n').length;
 return {method,status:'covered',native:name,path:definition.path,line,...count,...(method==='deferDependencyLaunch'?{scope:'Same transaction with dependency=true'}:{})};
});
console.log(JSON.stringify({status:'passed',publicMethods:154,directlyExercised:151,lifetimeCovered:1,laterBoundaries:2,dispatches:receipts.length,rows,claims:'Schema-26 durable methods and their bounded refusal/replay sequences only. Trace counts identify successful native example dispatches and do not imply complete branch coverage or actual external effects.'},null,2));
