/** Captured identity bytes must match JSON.stringify, including nested numbers and property order. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, mkdtempSync, openSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
const root=mkdtempSync(join(tmpdir(),'cr-json-'));
const values=['null','true','false','1.0','-0.0','1e21','1e20','1e-6','1e-7','1000000000000000100','9007199254740993','18446744073709551615','-9223372036854775808','5e-324','1.7976931348623157e308',
 '{"kind":"fixture","subjectId":"nested","value":{"10":10,"2":2,"01":1,"0":0,"4294967294":4,"4294967295":5,"-1":-1,"z":0,"a":1}}',
 '[{"b":1.0,"a":-0.0},null,[],{},"\\u0000\\b\\t\\n\\r\\f\\\\\\\"/日本語🦀\\u2028\\u2029"]'];
let seed=81921;const random=()=>seed=(Math.imul(seed,1664525)+1013904223)>>>0;
for(let i=0;i<6000;i++){const b=Buffer.alloc(8);b.writeUInt32LE(random(),0);b.writeUInt32LE(random(),4);const n=b.readDoubleLE();if(Number.isFinite(n))values.push(JSON.stringify(n));}
for(let i=0;i<400;i++){const a=String(random()%10000),b=String(random()%10000);values.push(`{"kind":"fixture","subjectId":"${i}","details":{"${a}":1.0,"x":[1e21,-0.0],"${b}":2.0}}`);}
const input='['+values.join(',')+']',expected=JSON.parse(input).map((v:any)=>JSON.stringify(v));
try {
 const path=join(root,'values.json');writeFileSync(path,input);const fd=openSync(path,'r');let actual:string[];
 try{actual=JSON.parse(execFileSync(resolve('target/debug/examples/json-oracle'),[],{stdio:[fd,'pipe','pipe'],encoding:'utf8',maxBuffer:5*1024*1024,timeout:30000}));}finally{closeSync(fd);}
 assert.equal(actual.length,expected.length);const failures=values.flatMap((value,i)=>actual[i]!==expected[i]?[{value,expected:expected[i],actual:actual[i]}]:[]);
 assert.equal(failures.length,0,'Serialization mismatches: '+JSON.stringify(failures.slice(0,12)));
 console.log(JSON.stringify({status:'passed',comparisons:values.length,corpusSha256:createHash('sha256').update(input).digest('hex'),covers:['normalized numeric spellings','ECMAScript exponent thresholds','safe/unsafe integer to Number conversion','finite IEEE-754 bit patterns','integer-key order and ordinary insertion order','nested arrays and scalar-safe string escaping'],claims:'Captured JSON record identity bytes; unsupported JavaScript-only values and lone UTF-16 surrogates are outside serde_json values.'},null,2));
}finally{rmSync(root,{recursive:true,force:true});}
