import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { savedInputs } from '../../server/runtime/inputs';
import { catalog, hostRedact, hostRoutes, hostTools, source } from './host-contract';

// Regeneration is explicit. An ordinary check never rewrites a drifting catalog.
assert.deepEqual(JSON.parse(readFileSync('crates/crewroom-runtime/src/host-tools.json','utf8')), catalog());
const cases: any[] = [], expected: any[] = [];
const flags = ['coordinator','refinement','research','researchRepository','publication','jira','reporter','researchCoordinatorReporter','coding','commandEngine','tools'];
const names = [...new Set(Object.values(catalog().definitions).map((tool:any)=>tool.name)), 'never_registered'];
for (let bits=0; bits<2**flags.length; bits++) {
  const config:any = { recipients:['Coordinator','Engineer 😀'] };
  for (const [i,flag] of flags.entries()) if (bits & 2**i) config[flag] = flag==='commandEngine' ? { socketPath:'/private/fixture/engine.sock' } : flag==='tools' ? [{id:'configured'},{id:'second-中'}] : true;
  const tools = hostTools(config), backend = hostRoutes(config);
  const registered = new Set(tools.map(t=>t.name)); const valid = registered.size===tools.length;
  const routes = names.map(name=> !valid || !registered.has(name) ? null : name==='run_command' ? 'command' : name==='run_check' ? 'check' : name==='run_shell' ? 'shell' : {backend:backend[name]});
  cases.push({op:'catalog',config,names}); expected.push({tools,backend,valid,routes});
}
const toolCases = cases.length;
const values: any[] = [null,true,12,'', '🔑secret中 secret secret-long', ['secret',{token:'secret',keep:'secret'}]];
for (const key of ['apiKey','APIKEY','headers','Headers','authorization','AUTHORIZATION','token','Token','providerPayload','PROVIDERPAYLOAD','api_key','tokens','authorizationX','ordinary','__proto__']) {
  const value = Object.fromEntries([[key,'secret'],['nested',[{[key]:{another:'secret'},value:'secret-long'}]]]); values.push(value);
}
for (const value of values) for (const secrets of [[],[''],['secret'],['secret-long','secret'],['secret','secret-long'],['🔑','中'],['[redacted]','secret']]) {
  cases.push({op:'redact',value,secrets});expected.push(hostRedact(value,secrets));
}
const redactions=cases.length-toolCases;
const root=mkdtempSync(join(tmpdir(),'crewroom-host-journal-oracle-'));
let inputCases=0;
function inputs(text:string) {
  writeFileSync(join(root,'inputs.jsonl'),text);let result;
  try { result={ok:true,inputs:savedInputs(root)}; } catch { result={ok:false}; }
  cases.push({op:'inputs',text});expected.push(result);inputCases++;
}
try {
  for(const disposition of ['accepted','received','completed','error','aborted','future']) for(const next of ['accepted','received','completed','error','aborted','future']) for(const end of ['','\n','\n\n']) {
    inputs([JSON.stringify({id:'first',message:'body 😀',disposition,extra:{retained:true}}),JSON.stringify({id:'second',message:'other',disposition:'accepted'}),JSON.stringify({id:'first',disposition:next,extra:{replacement:true}})].join('\n')+end);
  }
  for (const record of [null,{},[],1,'text',{id:1,disposition:'received'},{id:'key'},{id:'key',disposition:null},{id:'',disposition:'',extra:[1,true]}, {id:'key',disposition:'received',message:null}]) for(const prefix of ['', '{"id":"old","message":"saved","disposition":"completed"}\n']) inputs(prefix+JSON.stringify(record)+'\n');
  for(const text of ['', '\n', '\n\n','{','\r\n','null\n','{"id":"a","disposition":"accepted"}\n{"id":','{"id":"a","id":"b","disposition":"received"}\n','{"id":"a","disposition":"error","extra":-0.0}\n']) inputs(text);
  const serialized=JSON.stringify(cases), run=spawnSync('target/debug/examples/host-oracle',[],{input:serialized,encoding:'utf8',maxBuffer:128*1024*1024});
  assert.equal(run.status,0,run.stderr);const actual=JSON.parse(run.stdout);
  assert.equal(actual.length,expected.length);
  for(let i=0;i<actual.length;i++)assert.deepEqual(actual[i],JSON.parse(JSON.stringify(expected[i])),JSON.stringify({index:i,case:cases[i]}));
  console.log(JSON.stringify({status:'passed',toolConfigurations:toolCases,routeDecisions:toolCases*names.length,redactions,inputJournals:inputCases,comparisons:cases.length,corpusSha256:createHash('sha256').update(serialized).digest('hex'),retainedHostSha256:createHash('sha256').update(source).digest('hex'),boundary:'Complete metadata/routes/redaction and read-only input reconstruction. Registered-tool enforcement rejects routes unavailable to a role; overlapping schema registrations are refused. No independent host, runner or live delivery is claimed.'},null,2));
} finally { rmSync(root,{recursive:true,force:true}); }
