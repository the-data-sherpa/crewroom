#!/usr/bin/env python3
"""Three captured profiles and four-hop Mail round trip on actual native processes."""
import argparse,copy,hashlib,json,os,signal,socket,sqlite3,subprocess,sys,time,traceback
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Provider,Sandbox
from mailbox_contract_fixture import RustMail,NetworkGuard,free_port
PROJECT=Path(__file__).resolve().parents[2]
SELECTED=['coordinator','product-strategist','ux-principal','tech-writer']
NAMES=['Coordinator','Specialist1','Specialist2','Specialist3']

class ObservationProvider(Provider):
 """Only opt-in replies are chunked; ordinary profile protocol is unchanged."""
 def encode(self,provider,model,action):
  for event in super().encode(provider,model,action):
   if not action.get('nativeObservationStream'):
    yield event;continue
   if event.get('type')=='response.output_text.delta':text=event['delta'];kind='openai'
   elif event.get('type')=='content_block_delta' and event['delta'].get('type')=='text_delta':text=event['delta']['text'];kind='anthropic'
   elif provider=='aperture-google' and event.get('candidates'):text=event['candidates'][0]['content']['parts'][0]['text'];kind='google'
   else:yield event;continue
   size=max(1,(len(text)+11)//12);chunks=[text[i:i+size] for i in range(0,len(text),size)]
   for index,chunk in enumerate(chunks):
    item=copy.deepcopy(event)
    if kind=='openai':item['delta']=chunk
    elif kind=='anthropic':item['delta']['text']=chunk
    else:
     item['candidates'][0]['content']['parts'][0]['text']=chunk
     if index<len(chunks)-1:item['candidates'][0].pop('finishReason',None);item.pop('usageMetadata',None)
    time.sleep(.25);yield item

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--profile',choices=['baseline','heterogeneous','rotated'],required=True);p.add_argument('--binary-directory',type=Path,default=PROJECT/'target/debug');p.add_argument('--mail-binary',type=Path);p.add_argument('--playbooks',action='store_true',help='Verify coordinator checklist tools, skip validation and replay.');p.add_argument('--role-drafts',action='store_true',help='Verify recorded specialist drafting and operator approval.');p.add_argument('--principles',action='store_true',help='Verify copied principle text through actual runner input and a catalog edit.');p.add_argument('--model-jobs',action='store_true',help='Baseline profile through four inherited jobs and a live budget edit.');p.add_argument('--observe',action='store_true',help='Wait for private GUI/footprint control files, then run 60 seconds of bounded Unicode output; maximum driver time 10 minutes.');args=p.parse_args()
 if args.model_jobs and args.profile!='baseline':p.error('--model-jobs requires the baseline profile')
 binary_directory=args.binary_directory.resolve();binaries={}
 for name in ['crewroomd','crewroom-process','crewroom-host']:
  path=binary_directory/name
  if not path.is_file() or path.is_symlink() or not os.access(path,os.X_OK):p.error(f'Missing regular executable: {path}')
  binaries[name]={'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
 root=args.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
 catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())['models'];profiles=json.loads((PROJECT/'docs/testing/model-profiles.json').read_text());profile=next(p for p in profiles['profiles'] if p['id']==args.profile)
 roles={}
 for a in profile['assignments']:
  selection=profiles['modelPool'][a['modelKey']];model=next(m for m in catalog if m['selector']==selection['selector']);roles[a['roleId']]={**model,'effort':selection['effort'] or 'off'}
 jobs={'coordinator':'judgment','product-strategist':'code','ux-principal':'review','tech-writer':'fast'} if args.model_jobs else None
 if jobs:
  for role,job in jobs.items():roles[role]['effort']={'code':'high','judgment':'xhigh','review':'xhigh','fast':'low'}[job]
 provider=(ObservationProvider if args.observe else Provider)(reasoning=False);port=free_port();guard=NetworkGuard(root,[port],[port]);mail=RustMail(root/'mail',guard,port,binary=args.mail_binary)
 report={'passed':False,'profile':args.profile,'sourceRevision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=PROJECT,text=True).strip(),'binaries':binaries,'boundary':'Actual native daemon/OMP/tmux/Rust Mail; synthetic provider replies only.'};driver=None;sandbox=None;leases=[]
 try:
  mail.start();report['mail']=mail.identity()
  for _ in range(8):
   lease=socket.socket();lease.bind(('127.0.0.1',0));leases.append(lease)
  ports=[s.getsockname()[1] for s in leases];sandbox=Sandbox(root,ports+[provider.port]);report['guard']=sandbox.verify(provider.port)
  report['runner']={'version':subprocess.run([str(sandbox.binary),'--version'],capture_output=True,text=True,check=True).stdout.strip(),'sha256':hashlib.sha256(sandbox.binary.read_bytes()).hexdigest()}
  providers={};capabilities=[]
  for m in {roles[r]['selector']:roles[r] for r in SELECTED}.values():
   capabilities.append({'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*m['declaredThinkingEfforts']])),'tools':True})
   definition={'id':m['id'],'name':m['id'],'reasoning':m['declaredReasoning'],'input':m['declaredInput'],'contextWindow':m['declaredContextWindow'],'maxTokens':m['declaredMaxOutputTokens']}
   if m['declaredThinkingMode']:definition['thinking']={'mode':m['declaredThinkingMode'],'efforts':m['declaredThinkingEfforts']}
   providers.setdefault(m['provider'],{'baseUrl':f'http://127.0.0.1:{provider.port}/{{agent}}/{m["provider"]}/v1','api':m['api'],'apiKey':'synthetic-release-key','models':[]})['models'].append(definition)
  sent=set();draft_sent=set();playbook_stage=0;observation_requests={name:0 for name in NAMES};observation_output={name:{'replies':0,'lines':0,'utf8Bytes':0,'unicodeCharacters':0} for name in NAMES}
  def action(request):
   nonlocal playbook_stage
   name=request['agent'];body=json.dumps(request['body']);hop=None;to=None
   if args.playbooks and name=='Coordinator':
    if playbook_stage==0:
     playbook_stage=1;return {'kind':'tool','name':'playbook_status','arguments':{}}
    if playbook_stage==1:
     playbook_stage=2;return {'kind':'tool','name':'propose_playbook','arguments':{'playbookId':'fixture-flow','rationale':'Matches the bounded research fixture.'}}
    if 'Operator changed the advisory playbook' in body:
     if playbook_stage==2:
      playbook_stage=3;return {'kind':'tool','name':'record_playbook_step','arguments':{'version':1,'stepId':'inspect','state':'done'}}
     if playbook_stage==3:
      playbook_stage=4;return {'kind':'tool','name':'record_playbook_step','arguments':{'version':1,'stepId':'deliver','state':'skipped'}}
     if playbook_stage==4:
      playbook_stage=5;return {'kind':'tool','name':'record_playbook_step','arguments':{'version':1,'stepId':'deliver','state':'skipped','reason':'Research fixture has no delivery target.'}}
   if args.role_drafts and name=='Coordinator':
    for marker in ['ROLE_DRAFT_SUCCESS','ROLE_DRAFT_INVALID']:
     if marker in body:
      if marker in draft_sent:return {'kind':'text','text':'Draft proposal submitted for operator review.'}
      draft_sent.add(marker)
      role={'name':'Chart reviewer','category':'Quality','purpose':'Review scientific charts','instructions':'Check axes, units and uncertainty.','job':'review'}
      return {'kind':'tool','name':'propose_refinement','arguments':{'brief':json.dumps(role) if marker=='ROLE_DRAFT_SUCCESS' else 'invalid role JSON','criteria':['Reusable chart specialty'],'decisions':['Review before saving'],'unresolvedQuestions':[]}}
   if args.observe and 'NATIVE_OBSERVE_REQUEST'  in body and sent=={1,2,3,4}:
    observation_requests[name]+=1;number=observation_requests[name]
    output=''.join(f'Native observation · {name} · reply {number} · line {line}: café αβγ 漢字 🚀 — harmless scripted output.\n' for line in range(1,25))
    counts=observation_output[name];counts['replies']+=1;counts['lines']+=24;counts['utf8Bytes']+=len(output.encode());counts['unicodeCharacters']+=len(output)
    return {'kind':'text','text':output,'nativeObservationStream':True}
   if name=='Coordinator' and 1 not in sent:hop,to=1,'Specialist1'
   elif name=='Specialist1' and 'NATIVE_HOP_3' in body and 4 not in sent:hop,to=4,'Coordinator'
   elif name=='Specialist1' and 'NATIVE_HOP_1' in body and 2 not in sent:hop,to=2,'Specialist2'
   elif name=='Specialist2' and 'NATIVE_HOP_2' in body and 3 not in sent:hop,to=3,'Specialist1'
   if hop:
    sent.add(hop);return {'kind':'tool','name':'send_message','arguments':{'to':to,'body':f'NATIVE_HOP_{hop}'}}
   return {'kind':'text','text':'PROFILE_SETTLED'}
  for name in NAMES:provider.enqueue(name,*[action for _ in range(128 if args.observe else 16)])
  config={'root':str(root),'profile':args.profile,'binaryDirectory':str(binary_directory),'roles':roles,'selectedRoles':SELECTED,'runtime':{'ompBinary':str(sandbox.binary),'launchPrefix':['/usr/bin/sandbox-exec','-f',str(sandbox.path)],'hostPorts':ports,'catalog':capabilities,'models':{'providers':providers},'mail':{'url':f'http://127.0.0.1:{port}/mcp/','token':mail.token,'projectRoot':str(root/'mail-projects'),'retentionSeconds':86400,'atcDisabled':True}}}
  if args.playbooks:config['playbooks']={'id':'fixture-flow','name':'Research fixture','description':'Inspect and report','revision':1,'steps':[{'id':'inspect','title':'Inspect evidence'},{'id':'deliver','title':'Deliver result'}]}
  if args.role_drafts:config['roleDrafts']=True
  if args.principles:config['principles']={'id':'fixture-evidence','name':'Evidence','text':'PRINCIPLE_SNAPSHOT_ORIGINAL: cite exact tested revisions.','revision':1}
  if jobs:config['modelJobs']=jobs
  if args.observe:config['observation']={'mailPid':mail.proc.pid,'mailBinary':str(mail.binary),'harnessPid':os.getpid(),'loadMilliseconds':60000,'controlWaitMilliseconds':180000}
  path=root/'driver.json';path.write_text(json.dumps(config));path.chmod(0o600)
  for lease in leases:lease.close()
  with (root/'driver.log').open('w') as log:
   driver=subprocess.Popen(['node','--import','tsx','scripts/rust/release-profiles-live.ts',str(path)],cwd=PROJECT,env=dict(os.environ,CREWROOM_RETAIN_FIXTURE='1',CREWROOM_NATIVE_BINARY_DIRECTORY=str(binary_directory),GIT_EDITOR='true',GIT_SEQUENCE_EDITOR='true'),stdout=log,stderr=subprocess.STDOUT,start_new_session=True);driver.wait(timeout=600 if args.observe else 150)
  report['exitCode']=driver.returncode;report['passed']=driver.returncode==0
  if not report['passed']:print((root/'driver.log').read_text(),file=sys.stderr)
 except Exception:
  report['failure']=traceback.format_exc();print(report['failure'],file=sys.stderr)
 finally:
  if driver:
   try:os.killpg(driver.pid,0)
   except ProcessLookupError:pass
   else:
    report['passed']=False;report['forcedDriverCleanup']=True;os.killpg(driver.pid,signal.SIGKILL);driver.wait(timeout=10)
  states=[]
  for path in root.glob('cr-wf-*/runtime/*/host.json'):
   for name in ['host-process','omp-process']:
    owned=path.parent/name;check=subprocess.run([str(binary_directory/'crewroom-process'),'inspect',str(owned)],capture_output=True,text=True,timeout=15);state=json.loads(check.stdout);states.append({'path':str(owned),'state':state['state']})
    if state['state']!='stopped':
     report['passed']=False;subprocess.run([str(binary_directory/'crewroom-process'),'stop',str(owned)],capture_output=True,text=True,timeout=15)
  if args.principles:report['principlesInAllProviderInputs']=all(any(request['agent']==name and 'PRINCIPLE_SNAPSHOT_ORIGINAL' in json.dumps(request['body']) for request in provider.requests) for name in NAMES);report['passed']=report['passed'] and report['principlesInAllProviderInputs']
  report['ownedProcesses']=states;report['allOwnedProcessesExited']=len(states)==(12 if args.role_drafts else 8) and all(s['state']=='stopped' for s in states)
  for lease in leases:lease.close()
  mail.stop();provider.close();report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors
  expected={name:roles[role] for name,role in zip(NAMES,SELECTED)}
  report['observedEveryIdentity']=set(row['agent'] for row in provider.requests)==set(expected) and all(row['provider']==expected[row['agent']]['provider'] and row['model']==expected[row['agent']]['id'] for row in provider.requests)
  if args.observe:
   report['observationRequests']=observation_requests;report['observationOutput']=observation_output
   for name in ['observation-ready.json','load-started.json','load-complete.json','observation-error.json']:
    path=root/name
    if path.exists():report.setdefault('observationFiles',{})[name]={'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
   if (root/'load-complete.json').exists():report['observationLoad']=json.loads((root/'load-complete.json').read_text())
  report['providerTrace']=[{'agent':row['agent'],'provider':row['provider'],'model':row['model'],'path':row['path']} for row in provider.requests]
  (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
  if report['passed']:
   proof=json.loads((root/'profile-report.json').read_text());identities={a['assignmentId']:a for a in proof['identities']}
   with sqlite3.connect(f'file:{root / "mail/mail.sqlite3"}?mode=ro',uri=True) as db:
    rows=db.execute('SELECT m.id,m.subject,m.body_md,s.name,r.name,mr.ack_ts FROM messages m JOIN agents s ON s.id=m.sender_id JOIN message_recipients mr ON mr.message_id=m.id JOIN agents r ON r.id=mr.agent_id ORDER BY m.id').fetchall()
   report['externalMailReceipts']=[dict(zip(['transportId','messageId','body','sender','recipient','ackAt'],row)) for row in rows]
   report['externalMailMatches']=len(rows)==4 and all(row[0]==m['transportId'] and row[1]==m['id'] and row[2]==m['body'] and row[3]==identities[m['from']]['mailboxName'] and row[4]==identities[m['to']]['mailboxName'] and row[5] is not None for row,m in zip(rows,proof['messages']))
  report['binaryHashesUnchanged']=all(hashlib.sha256(Path(row['path']).read_bytes()).hexdigest()==row['sha256'] for row in binaries.values())
  if args.observe:report['passed']=report['passed'] and report.get('observationLoad',{}).get('passed',False) and all(n==report.get('observationLoad',{}).get('roundCount') and n>0 for n in observation_requests.values())
  report['passed']=report['passed'] and report['binaryHashesUnchanged'] and report.get('externalMailMatches',False) and report['allOwnedProcessesExited'] and report['observedEveryIdentity'] and not provider.errors
  if sandbox and report['allOwnedProcessesExited']:sandbox.close()
  for fixture in root.glob('cr-wf-*'):
   subprocess.run(['tmux','-S',str(fixture/'runtime/tmux.sock'),'kill-server'],capture_output=True,timeout=5)
  (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:v for k,v in report.items() if k not in ['providerTrace','ownedProcesses']},indent=2))
 return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
