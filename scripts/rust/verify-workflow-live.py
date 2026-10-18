#!/usr/bin/env python3
"""Full native workflow with actual OMP/tmux and Rust Mail; synthetic model replies only."""
import argparse,hashlib,json,os,signal,socket,subprocess,sys,traceback
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Provider,Sandbox
from mailbox_contract_fixture import RustMail,NetworkGuard,free_port
PROJECT=Path(__file__).resolve().parents[2]

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--service',action='store_true');parser.add_argument('--failure-mode',choices=['fallback','no-fallback','nonretryable'])
    args=parser.parse_args();root=args.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
    provider=Provider(reasoning=True);mailport=free_port();guard=NetworkGuard(root,[mailport],[mailport]);mail=RustMail(root/'mail',guard,mailport)
    report={'passed':False,'productionDaemon':args.service,'boundary':'Actual OMP/tmux, native workflow/hosts and Rust Mail; scripted model responses; no external provider delivery.'};driver=None;sandbox=None;leases=[]
    try:
        mail.start();report['mail']=mail.identity()
        for _ in range(4):
            lease=socket.socket();lease.bind(('127.0.0.1',0));leases.append(lease)
        ports=[s.getsockname()[1] for s in leases];sandbox=Sandbox(root,ports+[provider.port]);report['guard']=sandbox.verify(provider.port)
        report['runner']={'version':subprocess.run([str(sandbox.binary),'--version'],capture_output=True,text=True,check=True).stdout.strip(),'binarySha256':hashlib.sha256(sandbox.binary.read_bytes()).hexdigest()}
        catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())['models']
        # Explicit capability of these scripted provider fixtures, not a claim about live provider availability.
        capabilities=[{'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*(m['declaredThinkingEfforts'] or [])])),'tools':m['selector'] in ['aperture-openai/openai.gpt-6-sol','aperture-anthropic/anthropic.claude-sonnet-5']} for m in catalog]
        selections={};providers={}
        for name,selector,effort in [('coordinator','aperture-openai/openai.gpt-6-sol','low'),('specialist','aperture-anthropic/anthropic.claude-sonnet-5','medium')]:
            model=next(m for m in catalog if m['selector']==selector);selections[name]={'selector':selector,'effort':effort}
            definition={'id':model['id'],'name':model['id'],'reasoning':model['declaredReasoning'],'input':model['declaredInput'],'contextWindow':model['declaredContextWindow'],'maxTokens':model['declaredMaxOutputTokens']}
            if model['declaredThinkingMode']:definition['thinking']={'mode':model['declaredThinkingMode'],'efforts':model['declaredThinkingEfforts']}
            providers[model['provider']]={'baseUrl':f'http://127.0.0.1:{provider.port}/{name}/{model["provider"]}/v1','api':model['api'],'apiKey':'synthetic-upstream-key','models':[definition]}
        provider.enqueue('coordinator',{'kind':'tool','name':'send_message','arguments':{'to':'Specialist1','body':'LIVE_NATIVE_HANDOFF'}},*[{'kind':'text','text':'COORDINATOR_SETTLED'} for _ in range(6)])
        provider.enqueue('specialist',*[{'kind':'text','text':'SPECIALIST_SETTLED'} for _ in range(3)])
        if args.failure_mode:
            provider.actions['coordinator'].queue.clear()
            # The OMP adapter retries a 503 six times before reporting terminal failure.
            provider.enqueue('coordinator',*[{'kind':'error','status':401 if args.failure_mode=='nonretryable' else 503,'error':{'type':'authentication_error' if args.failure_mode=='nonretryable' else 'overloaded_error','message':'Synthetic provider failure'}} for _ in range(1 if args.failure_mode=='nonretryable' else 6)])
            provider.enqueue('specialist',*[{'kind':'text','text':'RECOVERED'} for _ in range(6)])
        config={'failureMode':args.failure_mode,'service':args.service,'root':str(root),'models':selections,'runtime':{'ompBinary':str(sandbox.binary),'launchPrefix':['/usr/bin/sandbox-exec','-f',str(sandbox.path)],'hostPorts':ports,'catalog':capabilities,'models':{'providers':providers},'mail':{'url':f'http://127.0.0.1:{mailport}/mcp/','token':mail.token,'projectRoot':str(root/'mail-projects'),'retentionSeconds':86400,'atcDisabled':True}}}
        if args.failure_mode=='fallback':config['runtime']['policy']={'fallbacks':{selections['coordinator']['selector']:[{'model':selections['specialist']['selector'],'effort':selections['specialist']['effort']}]}}
        path=root/'driver.json';path.write_text(json.dumps(config));path.chmod(0o600)
        for lease in leases:lease.close()
        env=dict(os.environ,CREWROOM_RETAIN_FIXTURE='1',GIT_EDITOR='true',GIT_SEQUENCE_EDITOR='true')
        driver=subprocess.Popen(['node','--import','tsx','scripts/rust/daemon-fallback-live.ts' if args.failure_mode else 'scripts/rust/workflow-live.ts',str(path)],cwd=PROJECT,env=env,stdout=(root/'driver.log').open('w'),stderr=subprocess.STDOUT,text=True,start_new_session=True)
        driver.wait(timeout=180);print((root/'driver.log').read_text(),end='');report['exitCode']=driver.returncode;report['passed']=driver.returncode==0
    except Exception:
        report['failure']=traceback.format_exc();print(report['failure'],file=sys.stderr)
    finally:
        if driver:
            try:os.killpg(driver.pid,0)
            except ProcessLookupError:pass
            else:
                report['passed']=False;report['forcedDriverCleanup']=True;os.killpg(driver.pid,signal.SIGKILL);driver.communicate(timeout=10)
        states=[]
        for path in root.glob('cr-wf-*/runtime/*/host.json'):
            for name in ['host-process','omp-process']:
                process=root/path.parent/name
                check=subprocess.run([str(PROJECT/'target/debug/crewroom-process'),'inspect',str(process)],capture_output=True,text=True,timeout=15)
                state=json.loads(check.stdout);states.append(state['state'])
                if state['state']!='stopped':
                    report['passed']=False;report.setdefault('fallbackCleanup',[]).append(str(process));subprocess.run([str(PROJECT/'target/debug/crewroom-process'),'stop',str(process)],capture_output=True,text=True,timeout=15)
        report['allOwnedProcessesExited']=bool(states) and all(s=='stopped' for s in states)
        for lease in leases:lease.close()
        mail.stop();provider.close();report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors
        (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
        if sandbox and report['allOwnedProcessesExited']:sandbox.close()
        report['passed']=report['passed'] and report['allOwnedProcessesExited'] and not provider.errors
        (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
