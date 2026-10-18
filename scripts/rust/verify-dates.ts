/** N2.7c: installed Node Date.parse is the source-date compatibility reference.
 * Each timezone runs in separate processes; no global TZ mutation or provider calls. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, mkdtempSync, openSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
const root=mkdtempSync(join(tmpdir(),'cr-date-'));
const groups=new Map<string,Set<string>>();
function add(group:string,...values:string[]) {let set=groups.get(group);if(!set){set=new Set();groups.set(group,set);}for(const v of values)set.add(v);}
add('fixed','', 'garbage','0','1','12','13','49','50','99','100','00000','-000000','-000000-01-01','+000000-01-01',
 'Jan 1','Jan 01 70','February 31, 2026','Sat, 02 Oct 2026 03:04:05 GMT','Fri Oct 02 2026 00:00:00 GMT-0400 (Eastern Daylight Time)',
 '2026-10-02\0junk','2026-10-02\0T25:00','2026-10-02T00:00:00.000000001Z','2026-10-02T24:00:00.000000001Z');
for(const year of ['0000','0001','0099','0100','1900','2000','2024','2026','9999','+010000','-000001','+275760','-271821','+999999'])
 for(const month of ['01','02','04','10','12','00','13']) for(const day of ['01','28','29','30','31','00','32']) {
  const date=`${year}-${month}-${day}`;add('calendar',date,date+'T00:00:00Z',date+'T00:00:00');
 }
for(const year of ['0','01','12','13','49','50','99','100','999','2026','00000','+002026','-002026','-000000'])
 for(const separator of ['-','/','.',',',' ','--'])for(const date of [year,year+separator+'2',year+separator+'2'+separator+'3'])add('legacy numeric',date);
for(const h of ['0','00','12','23','24','25'])for(const minute of ['0','00','30','59','60'])for(const suffix of ['',':00',':30',':60','.1',':00.1',':00.0000',':00.000000001',':00.1234567891234'])
 for(const zone of ['','Z','z','+0000','-04:00','+24:00','+023',' GMT'])add('time',`2026-10-02T${h}:${minute}${suffix}${zone}`);
for(const text of ['2026-10-02','2026','Oct 2 2026','2026-10-02T00:00:00Z','2026-10-02 01:02:03'])
 for(const pad of [' ','\t','\n','\r','\v','\f','\u0085','\u00a0','\u1680','\u2000','\u2028','\u2029','\u202f','\u205f','\u3000','\ufeff','(comment)','(nested (comment))','(unfinished','junk',')','+','-'])add('padding',pad+text,text+pad,pad+text+pad);
for(const month of ['Jan','FEBRUARY','Marching','Apr','May','June','July','Aug','Sep','October','November','Dec','Foo'])
 for(const shape of [`${month} 2 2026`,`2-${month}-26`,`2026 ${month} 2`,`2 2026 ${month}`,`${month} 2026`,`2 ${month}`])
 for(const time of ['',' 1:02',' 1::',' 1:02:03.4',' 12:00 AM',' 12:00 PM',' 00:00 PM',' 13:00 PM',' 12:00am',' 12:00:00am',' 24:00',' 25:00'])add('legacy words',shape+time);
for(const zone of ['UT','UTC','GMT','Z','EST','EDT','CST','CDT','MST','MDT','PST','PDT','CET','GMT+5','GMT-8','GMT+0530','GMT-03:30','GMT+24','GMT+99','GMT+9999','GMT+999999999:59','GMT+0:99','GMT+05:','+0100','-0100','GMT-000000'])
 for(const form of ['Oct 2 2026','Oct 2 2026 12:00:00','2026-10-02 12:00:00'])add('legacy zones',form+' '+zone);
for(const date of ['2026-03-08','2026-11-01','2026-03-29','2026-10-25','2026-04-05','2026-10-04','2011-12-30','1883-11-18','1900-01-01','1945-08-14','2038-03-14'])
 for(const h of ['00','01','02','03','12','23'])for(const m of ['00','15','30','45','59'])add('local transitions',date+`T${h}:${m}:00`);
for(const s of ['+275760-09-13T00:00:00.000Z','+275760-09-13T00:00:00.001Z','-271821-04-20T00:00:00.000Z','-271821-04-19T23:59:59.999Z','+275760-09-13','-271821-04-20'])add('clip',s,s.replace('Z','+01:00'),s.replace('Z','-01:00'));
let seed=91371;const random=(n:number)=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed%n;};
const parts=['2026','01','02','31','99','0000','9999999999999','T','Z','GMT','Oct','am','pm','foo',':','-','+','.','/',' ',')','(ignored)','\u2028'];
for(let i=0;i<2500;i++)add('deterministic malformed',Array.from({length:1+random(12)},()=>parts[random(parts.length)]).join(''));
const cases=[...new Set([...groups.values()].flatMap(s=>[...s]))];
const input=JSON.stringify(cases);const file=join(root,'dates.json');writeFileSync(file,input);
function run(command:string,args:string[],env:NodeJS.ProcessEnv) {const fd=openSync(file,'r');try{return JSON.parse(execFileSync(command,args,{env,stdio:[fd,'pipe','pipe'],encoding:'utf8',maxBuffer:5*1024*1024,timeout:30000}));}finally{closeSync(fd);}}
try {
 const zones=['UTC','America/New_York','Europe/Berlin','Asia/Kolkata','Australia/Lord_Howe','Pacific/Apia'];const results=[];
 for(const zone of zones){const env={...process.env,TZ:zone};const expected=run(process.execPath,['-e',"let s='';process.stdin.setEncoding('utf8');process.stdin.on('data',x=>s+=x);process.stdin.on('end',()=>console.log(JSON.stringify(JSON.parse(s).map(x=>{const n=Date.parse(x);return Number.isFinite(n)?n:null;}))));"],env);const actual=run(resolve('target/debug/examples/date-oracle'),[],env);
  assert.equal(actual.length,cases.length);const failures=cases.flatMap((value,i)=>actual[i]!==expected[i]?[{value,expected:expected[i],actual:actual[i]}]:[]);
  assert.equal(failures.length,0,zone+' mismatches: '+JSON.stringify(failures.slice(0,25)));
  results.push({zone,cases:cases.length,accepted:expected.filter((x:any)=>x!==null).length});
 }
 console.log(JSON.stringify({status:'passed',comparisons:cases.length*results.length,uniqueDates:cases.length,corpusSha256:createHash('sha256').update(input).digest('hex'),groups:Object.fromEntries([...groups].map(([name,set])=>[name,set.size])),zones:results,reference:{node:process.version,v8:process.versions.v8,icu:process.versions.icu,tz:process.versions.tz},claims:'Date.parse parity only, in isolated timezone processes; source text remains unchanged in storage.'},null,2));
}finally{rmSync(root,{recursive:true,force:true});}
