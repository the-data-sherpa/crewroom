#!/usr/bin/env python3
"""Real OMP -> native gateway -> scripted provider, with an OS network guard."""
import argparse,hashlib,importlib.util,json,queue,subprocess,sys,threading,traceback,uuid
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Provider,Sandbox,wait_until
module=importlib.util.spec_from_file_location('native_rpc',Path(__file__).with_name('verify-omp-rpc.py'));rpc=importlib.util.module_from_spec(module);module.loader.exec_module(rpc)
PROJECT=Path(__file__).resolve().parents[2]
class Gateway:
    def __init__(self,root,plan,provider):
        self.root=root;root.mkdir(parents=True);self.frames=[];self.pending={};self.ready=queue.Queue();self.error=(root/'stderr.log').open('w');self.log=(root/'gateway.jsonl').open('w')
        self.proc=subprocess.Popen([str(PROJECT/'target/debug/examples/gateway-fixture')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.error,text=True)
        self.proc.stdin.write(json.dumps({'root':str(root),'plan':plan,'provider':provider})+'\n');self.proc.stdin.flush()
        self.thread=threading.Thread(target=self.read,daemon=True);self.thread.start()
        try:self.origin=self.ready.get(timeout=10)['origin'];self.port=int(self.origin.rsplit(':',1)[1])
        except Exception:
            self.proc.terminate()
            try:self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:self.proc.kill();self.proc.wait(timeout=5)
            self.thread.join(2);self.proc.stdin.close();self.proc.stdout.close();self.log.close();self.error.close()
            raise
    def read(self):
        for line in self.proc.stdout:
            self.log.write(line);self.log.flush();value=json.loads(line);self.frames.append(value)
            if value.get('ready'):self.ready.put(value)
            elif value.get('id') in self.pending:self.pending[value['id']].put(value)
    def command(self,op):
        ident=uuid.uuid4().hex;target=self.pending[ident]=queue.Queue();self.proc.stdin.write(json.dumps({'op':op,'id':ident})+'\n');self.proc.stdin.flush()
        try:return target.get(timeout=10)['result']
        finally:self.pending.pop(ident,None)
    def close(self):
        if self.proc.poll() is None:self.command('close');self.proc.stdin.close();self.proc.wait(timeout=10)
        self.thread.join(2);self.log.close();self.error.close();self.proc.stdout.close();assert self.proc.returncode==0
    @property
    def events(self):return [v['event'] for v in self.frames if 'event' in v]

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);root=parser.parse_args().output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
    catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    capabilities=[{'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*(m['declaredThinkingEfforts'] or [])])),'tools':None} for m in catalog['models']]
    base={'id':'fixture','name':'Fixture','category':'Test','purpose':'Gateway routing','instructions':'Return fixture markers.','enabled':True}
    provider=Provider(reasoning=True);gateways=[];agents=[];sandbox=None;report={'cases':[]}
    try:
        definitions=[]
        for i,(selector,effort) in enumerate([('aperture-openai/openai.gpt-6-sol','low'),('aperture-anthropic/anthropic.claude-sonnet-5','medium'),('aperture-google/gemini-3.8-flash','low'),('aperture-openai/openai.gpt-6-sol','low')]):
            name=f'gateway-{i}';plan=rpc.policy({**base,'model':selector,'effort':effort},capabilities);m=next(m for m in catalog['models'] if m['selector']==selector)
            definition={'baseUrl':f'http://127.0.0.1:{provider.port}/{name}/{m["provider"]}/v1','apiKey':'synthetic-upstream-key','models':[{'id':m['id'],'contextWindow':m['declaredContextWindow'],'maxTokens':m['declaredMaxOutputTokens']}]}
            gateway=Gateway(root/name/'gateway',plan,definition);gateways.append(gateway);definitions.append((name,plan,selector,m,gateway))
        sandbox=Sandbox(root,[provider.port,*[g.port for g in gateways]]);report['guard']=sandbox.verify(provider.port)
        report['runner']=sandbox.identity()
        for i,(name,plan,selector,model,g) in enumerate(definitions):
            def configure(private,g=g):
                path=private/'models.yml';models=json.loads(path.read_text())
                for value in models['providers'].values():
                    value['baseUrl']=g.origin+'/provider'+value['baseUrl'].split(f':{provider.port}',1)[1];value['apiKey']='host-token'
                path.write_text(json.dumps(models))
            a=rpc.NativeAgent(name,root,sandbox,provider,catalog,selector,settings=rpc.settings(plan),config_hook=configure);agents.append(a);a.command('native_configure',plan=plan)
            before=len(provider.requests)
            if i<3:
                a.text_turn(provider,'FIRST_NATIVE_GATEWAY');a.text_turn(provider,'SECOND_NATIVE_GATEWAY')
                wait_until(lambda:len([e for e in g.events if e['type']=='provider_usage'])==2)
                usage=[e for e in g.events if e['type']=='provider_usage'];assert all(e['state']=='actual' for e in usage),usage
                assert len(provider.requests)-before==2 and g.command('state')['failure'] is None
                if i==0:
                    budget=json.loads((g.root/'reasoning-budget.json').read_text());assert budget and all(isinstance(v,(int,float)) and v>0 for v in budget.values())
                    budgets=[e for e in g.events if e['type']=='provider_budget'];assert budgets[-1]['opaqueTokens']>0,budgets
                assert 'synthetic-upstream-key' not in json.dumps(g.events)
                report['cases'].append({'name':'two real OMP turns through native gateway with actual synthetic usage','selector':selector,'providerRequests':2,'usage':usage,'status':'passed'})
            else:
                start=len(a.frames)
                provider.enqueue(name,{'kind':'error','status':400,'error':{'type':'invalid_request_error','param':'reasoning.effort','message':"Unsupported reasoning_effort 'low'. Supported values are 'medium'."}},{'kind':'text','text':'UNAUTHORIZED_EFFORT_COMPLETED'})
                a.command('prompt',message='Use only the admitted effort.')
                result=a.wait_frame('prompt_result',start,timeout=30)
                assert result['status']=='error',result
                assert len(provider.requests)-before==1
                assert any(e['type']=='model_policy_rejected' for e in g.events)
                assert 'UNAUTHORIZED_EFFORT_COMPLETED' not in json.dumps(a.frames)
                report['cases'].append({'name':'OMP effort substitution blocked before a second upstream request','providerRequests':1,'status':'passed'})
            a.close();g.close()
        report['passed']=True
    except Exception as error:report['passed']=False;report['error']=str(error);traceback.print_exc()
    finally:
        for a in agents:
            if not a.log.closed:a.close()
        for g in gateways:
            if not g.log.closed:g.close()
        provider.close();report['allOwnedProcessesExited']=all(a.proc.poll() is not None and a.inspect()['state']=='stopped' for a in agents) and all(g.proc.poll() is not None for g in gateways)
        report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors
        (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
        if sandbox and report['allOwnedProcessesExited']:sandbox.close()
        report['passed']=report.get('passed',False) and report['allOwnedProcessesExited'] and not provider.errors
        (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return 0 if report['passed'] else 1
if __name__=='__main__':raise SystemExit(main())
