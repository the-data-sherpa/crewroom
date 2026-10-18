#!/usr/bin/env python3
"""Original complex/drift/rework acceptance sequences on actual native services and OMP."""
import argparse,copy,hashlib,json,os,signal,sqlite3,subprocess,sys,traceback
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from docker_engine import docker_socket
from omp_offline_fixture import Provider,Sandbox
from mailbox_contract_fixture import RustMail,free_port
from complex_fixture import enqueue_complex,enqueue_drift
from coding_fixture import enqueue_coding
PROJECT=Path(__file__).resolve().parents[2]


def final_package_receipt(binary_directory,binaries,binary_source):
 """Bind actual runtime bytes to final signed package provenance, never pre-sign hashes."""
 internal=binary_directory.parent/'Resources/build-receipt.json'
 bundle=binary_directory.parent.parent
 packaged=binary_directory.name=='MacOS' and binary_directory.parent.name=='Contents' and bundle.suffix=='.app'
 if not packaged and not internal.exists():return None
 if not binary_source or len(binary_source)!=40 or any(c not in '0123456789abcdef' for c in binary_source):raise ValueError('A packaged scenario requires --binary-source with the exact full candidate SHA')
 sidecar=bundle.with_name(bundle.name+'.receipt.json')
 for path in [internal,sidecar]:
  if not path.is_file() or path.is_symlink():raise ValueError(f'Missing regular package receipt: {path}')
 original=json.loads(internal.read_text());final=json.loads(sidecar.read_text())
 if original.get('sourceRevision')!=binary_source or final.get('sourceRevision')!=binary_source:raise ValueError('Internal/final package source revision differs from --binary-source')
 internal_hash=hashlib.sha256(internal.read_bytes()).hexdigest()
 if final.get('internalProvenanceSha256')!=internal_hash:raise ValueError('Final sidecar does not bind the internal provenance bytes')
 if final.get('bundleSignatureVerified') is not True:raise ValueError('Final sidecar does not record completed bundle signature verification')
 verified={}
 for name in ['crewroomd','crewroom-process','crewroom-host']:
  row=binaries[name];expected=final.get('binaries',{}).get(name,{}).get('packagedSha256')
  if row['path']!=str(binary_directory/name) or row['sha256']!=expected:raise ValueError(f'Packaged binary differs from final signed sidecar: {name}')
  verified[name]=expected
 return {'path':str(sidecar),'sha256':hashlib.sha256(sidecar.read_bytes()).hexdigest(),'sourceRevision':binary_source,'verifiedPackagedBinaries':verified,'internalProvenance':{'path':str(internal),'sha256':internal_hash,'scope':'Source/pre-signature provenance only; executable hashes above come from the final signed sidecar.'}}

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('--output',type=Path,required=True)
 parser.add_argument('--scenario',choices=['complex','target-drift','rework'],required=True)
 parser.add_argument('--binary-directory',type=Path,default=PROJECT/'target/debug')
 parser.add_argument('--provider-daemon',type=Path,help='Existing native Service example compiled from the candidate source/profile; required for loopback publication only.')
 parser.add_argument('--mail-binary',type=Path)
 parser.add_argument('--browser-image',help='Immutable no-volume Chromium image from prepare-browser-check-image.py; required for complex/drift.')
 parser.add_argument('--binary-source',help='Exact candidate source SHA, recorded separately from fixture source; required for packages and verified against the adjacent final signed .app.receipt.json.')
 args=parser.parse_args();binary_directory=args.binary_directory.resolve();binaries={}
 paths={name:binary_directory/name for name in ['crewroomd','crewroom-process','crewroom-host']}
 if args.scenario!='rework':paths['provider-daemon']=(args.provider_daemon or binary_directory/'examples/provider-daemon').resolve()
 for name,path in paths.items():
  if not path.is_file() or path.is_symlink() or not os.access(path,os.X_OK):parser.error(f'Missing regular executable: {path}')
  binaries[name]={'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
 try:package_proof=final_package_receipt(binary_directory,binaries,args.binary_source)
 except (ValueError,KeyError,TypeError) as error:parser.error(str(error))
 browser_image=None
 if args.scenario!='rework':
  if not args.browser_image or not args.browser_image.startswith('sha256:'):parser.error('--browser-image must name the prepared immutable image')
  browser_image=json.loads(subprocess.check_output(['docker','image','inspect',args.browser_image],text=True))[0]
  if browser_image['Id']!=args.browser_image or browser_image['Config'].get('Volumes'):parser.error('Browser image must have exact immutable identity and no inherited volumes')
  original=json.loads(subprocess.check_output(['docker','image','inspect','sha256:425dd73955b5b99e61a37954480a0de825d203b55f8fda4d4377f4e29fdb8ea7'],text=True))[0]
  expected_config=copy.deepcopy(original['Config']);expected_config.pop('Volumes',None)
  if browser_image['RootFS']!=original['RootFS'] or browser_image['Config']!=expected_config:parser.error('Derived Chromium image must retain the original layers and every other image setting')
 root=args.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700);data=root/'data';data.mkdir(mode=0o700)
 report={'passed':False,'scenario':args.scenario,'fixtureSource':subprocess.check_output(['git','rev-parse','HEAD'],cwd=PROJECT,text=True).strip(),'binarySource':args.binary_source,'binaries':binaries,'boundary':'Actual native Service/runtime, OMP, Mail, Git and Docker. Existing provider-daemon injects only synthetic loopback Bitbucket credentials/endpoints; rework uses crewroomd. No live provider delivery or model-quality claim.'}
 if package_proof:report['finalPackageReceipt']=package_proof
 report['fixtureInputs']={str(path.relative_to(PROJECT)):hashlib.sha256(path.read_bytes()).hexdigest() for path in [Path(__file__).resolve(),*[PROJECT/'scripts/rust'/name for name in ['native-complex-live.ts','native-rework-live.ts','native-scenario-backend.ts','daemon-client.ts']],*[PROJECT/'scripts'/name for name in ['complex_fixture.py','coding_fixture.py','omp_offline_fixture.py','mailbox_contract_fixture.py','runtime-observer.ts','delivery-provider-fixture.ts']],*sorted((PROJECT/'docs/testing/fixtures/saved-filter-app').glob('*'))]}
 if browser_image:report['browserImage']={'id':browser_image['Id'],'rootFilesystem':browser_image['RootFS'],'volumes':browser_image['Config'].get('Volumes'),'preparationScriptSha256':hashlib.sha256((PROJECT/'scripts/rust/prepare-browser-check-image.py').read_bytes()).hexdigest()}
 provider=Provider();mail_port=free_port();server_port=free_port();host_ports=[free_port() for _ in range(8)]
 sandbox=Sandbox(root,[provider.port,mail_port,server_port,*host_ports],[mail_port,*host_ports]);mail=RustMail(root/'mail',sandbox,mail_port,binary=args.mail_binary)
 models=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())['models']
 selectors=['aperture-openai/openai.gpt-6-sol','aperture-anthropic/anthropic.claude-sonnet-5','aperture-google/gemini-3.8-flash'];efforts=['low','medium','low'];selected=[];capabilities=[];providers={}
 for selector,effort in zip(selectors,efforts):
  m=next(m for m in models if m['selector']==selector);selected.append({**m,'effort':effort})
  capabilities.append({'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*m['declaredThinkingEfforts']])),'tools':True})
  definition={'id':m['id'],'name':m['id'],'reasoning':m['declaredReasoning'],'input':m['declaredInput'],'contextWindow':m['declaredContextWindow'],'maxTokens':m['declaredMaxOutputTokens']}
  if m['declaredThinkingMode']:definition['thinking']={'mode':m['declaredThinkingMode'],'efforts':m['declaredThinkingEfforts']}
  providers[m['provider']]={'api':m['api'],'apiKey':'synthetic-native-scenario-key','baseUrl':f'http://127.0.0.1:{provider.port}/{{agent}}/{m["provider"]}/v1','models':[definition]}
 roles={role:selected[i] for role,i in [('triage',0),('implementer',1),('reviewer',2)]}
 runtime={'root':str(root/'hosts'),'socket':str(root/'hosts/tmux.sock'),'ompBinary':str(sandbox.binary),'launchPrefix':['/usr/bin/sandbox-exec','-f',str(sandbox.path)],'hostPorts':host_ports,'mail':{'url':f'http://127.0.0.1:{mail_port}/mcp/','token':mail.token,'projectRoot':mail.project,'retentionSeconds':86400,'atcDisabled':True},'commandEngine':{'socket':docker_socket()},'catalog':capabilities,'policy':{},'models':{'providers':providers}}
 config={'root':str(data),'rolePath':str(data/'roles.json'),'serverPort':server_port,'binaryDirectory':str(binary_directory),'providerFixtureBinary':str(paths.get('provider-daemon','')),'runtime':runtime,'selectedRoles':roles,'targetDrift':args.scenario=='target-drift','browserImage':args.browser_image}
 path=root/'driver.json';path.write_text(json.dumps(config));path.chmod(0o600)
 if args.scenario=='rework':enqueue_coding(provider,'rework')
 else:
  enqueue_complex(provider)
  if args.scenario=='target-drift':enqueue_drift(provider,data)
 driver=None
 try:
  report['guard']=sandbox.verify(provider.port);mail.start();report['mail']=mail.identity();report['runner']={'version':subprocess.check_output([str(sandbox.binary),'--version'],text=True).strip(),'sha256':hashlib.sha256(sandbox.binary.read_bytes()).hexdigest()}
  script='native-rework-live.ts' if args.scenario=='rework' else 'native-complex-live.ts'
  with (root/'driver.log').open('w') as log:
   driver=subprocess.Popen(['node','--import','tsx','scripts/rust/'+script,str(path)],cwd=PROJECT,env=dict(os.environ,CREWROOM_NATIVE_BINARY_DIRECTORY=str(binary_directory),GIT_EDITOR='true',GIT_SEQUENCE_EDITOR='true',CREWROOM_BITBUCKET_TOKEN='synthetic-native-fixture'),stdout=log,stderr=subprocess.STDOUT,start_new_session=True);driver.wait(timeout=420)
  report['exitCode']=driver.returncode;report['passed']=driver.returncode==0
  if not report['passed']:print((root/'driver.log').read_text(),file=sys.stderr)
 except Exception:
  report['failure']=traceback.format_exc();print(report['failure'],file=sys.stderr)
 finally:
  if driver and driver.poll() is None:report['passed']=False;report['forcedDriverCleanup']=True;os.killpg(driver.pid,signal.SIGKILL);driver.wait(timeout=10)
  states=[]
  for path in (root/'hosts').glob('*/host.json'):
   for name in ['host-process','omp-process']:
    owned=path.parent/name;result=subprocess.run([str(binary_directory/'crewroom-process'),'inspect',str(owned)],capture_output=True,text=True,timeout=15);state=json.loads(result.stdout);states.append({'path':str(owned),'state':state['state']})
    if state['state']!='stopped':report['passed']=False;subprocess.run([str(binary_directory/'crewroom-process'),'stop',str(owned)],capture_output=True,text=True,timeout=15)
  report['ownedProcesses']=states;report['allOwnedProcessesExited']=bool(states) and all(s['state']=='stopped' for s in states)
  containers=[]
  for path in (root/'hosts').glob('*/tools/*/container.json'):
   intent=json.loads(path.read_text());row={'path':str(path),'id':intent.get('id'),'removed':intent.get('removed',False)}
   if intent.get('id') and not row['removed']:
    report['passed']=False
    inspected=subprocess.run(['docker','inspect',intent['id']],capture_output=True,text=True,timeout=15)
    if inspected.returncode==0:
     actual=json.loads(inspected.stdout)[0]
     if actual['Id']!=intent['id'] or actual['Config']['Labels'].get('io.crewroom.command')!=intent['token'] or actual['Image']!=intent['spec']['image']:raise RuntimeError('Refusing cleanup of unmatched container identity')
     if actual['State']['Running']:subprocess.run(['docker','kill',intent['id']],check=True,capture_output=True,timeout=15)
     subprocess.run(['docker','rm','-v',intent['id']],check=True,capture_output=True,timeout=15);row['fallbackRemovedWithAnonymousVolumes']=True
   containers.append(row)
  report['containers']=containers
  mail.stop();provider.close();report['providerErrors']=provider.errors;report['providerRequests']=len(provider.requests)
  expected={'Coordinator':selected[0],'SpecialistOne':selected[1],'SpecialistTwo':selected[2]}
  if args.scenario!='rework':expected['Guide']=selected[0]
  report['observedEveryIdentity']=set(r['agent'] for r in provider.requests)==set(expected) and all(r['provider']==expected[r['agent']]['provider'] and r['model']==expected[r['agent']]['id'] for r in provider.requests)
  report['providerTrace']=[{'agent':r['agent'],'provider':r['provider'],'model':r['model'],'path':r['path']} for r in provider.requests]
  (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
  if (data/'scenario-report.json').exists():
   report['proof']=json.loads((data/'scenario-report.json').read_text())
   with sqlite3.connect(f'file:{root / "mail/mail.sqlite3"}?mode=ro',uri=True) as db:rows=db.execute('SELECT m.id,m.subject,m.body_md,mr.ack_ts FROM messages m JOIN message_recipients mr ON mr.message_id=m.id ORDER BY m.id').fetchall()
   report['externalMailReceipts']=[dict(zip(['transportId','messageId','body','ackAt'],row)) for row in rows]
   messages=report['proof']['messages'];report['externalMailMatches']=len(rows)==len(messages) and all(any(row[0]==m['transportId'] and row[1]==m['id'] and row[2]==m['body'] and row[3] is not None and m['disposition']=='received' for m in messages) for row in rows)
  report['binaryHashesUnchanged']=all(hashlib.sha256(Path(row['path']).read_bytes()).hexdigest()==row['sha256'] for row in binaries.values())
  report['passed']=report['passed'] and report['allOwnedProcessesExited'] and report['observedEveryIdentity'] and report['binaryHashesUnchanged'] and report.get('externalMailMatches',False) and not provider.errors
  if report['allOwnedProcessesExited']:sandbox.close()
  subprocess.run(['tmux','-S',str(root/'hosts/tmux.sock'),'kill-server'],capture_output=True,timeout=5)
  (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:v for k,v in report.items() if k not in ['providerTrace','ownedProcesses','proof']},indent=2))
 return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
