/** The process killed here owns the native SQLite connection, not a TypeScript store. */
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {NativeMail} from './mail-adapter';
const config=JSON.parse(readFileSync(process.argv[2],'utf8'));
const [messageId,recipientAttemptId,window]=process.argv.slice(3);
const client=new NativeMail(config);
try {
  await client.ready;
  if(window!=='before-persistence')await client.command('accept_local',{target:messageId});
  if(window==='before-ack'||window==='after-ack'){
    const response=await fetch(config.controlUrl+`/drop-ack-${window==='before-ack'?'before':'after'}`,{method:'POST',signal:AbortSignal.timeout(10000)});assert.ok(response.ok);await assert.rejects(client.command('accept',{target:messageId}));
  }
  if(window==='after-processing'){
    await client.command('accept',{target:messageId});await client.command('dispatch',{target:messageId});await client.command('prepare',{target:recipientAttemptId,key:`effect-${messageId}`,input:{to:'Coordinator',body:`Effect for ${messageId}`,replyTo:messageId}});
  }
  client.proc.kill('SIGKILL');const [code,signal]=await client.exit;assert.equal(code,null);assert.equal(signal,'SIGKILL');console.log(JSON.stringify({passed:true,pid:client.proc.pid,signal,window}));
}finally{await client.close();}
