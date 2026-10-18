/** Explicit store seed for Mail tests: assignments are records, not executing agents. */
import {mkdtempSync,realpathSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {Workbench} from '../../server/workbench';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
import {seedRepository,saveRepositoryFixture} from '../../tests/fixtures/repository';
export function seedMessages(directory?:string) {
  const root=directory??realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-mail-')));seedRepository(root);const database=join(root,'workflow.sqlite');const store=new Workbench(database,()=>1000);
  try {
    const repository=saveRepositoryFixture(store,{name:'Mail fixture',path:root,targetBranch:'main'});const task=store.createTask({title:'External messages',description:'',repositoryId:repository.id});
    store.refine(task.id,{revision:1,sourceRevision:1,brief:'Exchange durable messages',criteria:['Preserve each logical effect']});const run=store.queue(task.id,{revision:2});
    const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
    const sender=store.admitAssignment(run.id,{name:'Coordinator',role,model,coordinator:true,input:'Coordinate.'});const recipient=store.admitAssignment(run.id,{name:'Specialist',role,model,coordinator:false,input:'Investigate.'});
    return {root,database,task,run,sender,recipient};
  } finally {store.close();}
}
