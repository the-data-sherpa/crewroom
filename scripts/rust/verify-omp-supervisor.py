#!/usr/bin/env python3
"""Actual guarded OMP through native supervisor launch/observation/restart/stop.
Provider responses and the message backend are explicit loopback fixtures.
"""
import argparse,hashlib,http.server,importlib.util,json,os,queue,signal,socket,subprocess,sys,threading,traceback,uuid
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Provider,Sandbox,wait_until
module=importlib.util.spec_from_file_location('native_rpc',Path(__file__).with_name('verify-omp-rpc.py'));rpc=importlib.util.module_from_spec(module);module.loader.exec_module(rpc)
PROJECT=Path(__file__).resolve().parents[2];BIN=PROJECT/'target/debug'

class Backend:
    def __init__(self,hosts):
        self.requests=[];owner=self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                value=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                config=json.loads((hosts/value['attemptId']/'host.json').read_text())
                assert self.path=='/internal/agent-message'
                assert self.headers['Authorization']=='Bearer '+config['token'] and self.headers['Origin']==owner.origin
                owner.requests.append(value);self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"stored":true}')
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);self.origin=f'http://127.0.0.1:{self.server.server_port}'
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
    def close(self):self.server.shutdown();self.server.server_close();self.thread.join(2)

class Controller:
    def __init__(self,config,log):
        self.responses={};self.ready=threading.Event();self.log=log.open('w');self.closed=False
        self.proc=subprocess.Popen([str(BIN/'examples/supervisor-fixture')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.log,text=True,cwd=PROJECT)
        def read():
            for line in self.proc.stdout:
                value=json.loads(line)
                if value.get('ready'):self.ready.set()
                elif value.get('id') in self.responses:self.responses[value['id']].put(value)
        self.thread=threading.Thread(target=read,daemon=True);self.thread.start();self.proc.stdin.write(json.dumps(config)+'\n');self.proc.stdin.flush()
        assert self.ready.wait(10),'Native controller failed startup; inspect '+str(log)
    def call(self,op,**fields):
        ident=uuid.uuid4().hex;reply=queue.Queue();self.responses[ident]=reply
        self.proc.stdin.write(json.dumps({'id':ident,'op':op,**fields})+'\n');self.proc.stdin.flush()
        try:
            value=reply.get(timeout=45)
            if 'error' in value:raise RuntimeError(value['error'])
            return value.get('value')
        finally:self.responses.pop(ident,None)
    def kill(self):
        self.proc.kill();self.proc.wait(5);self.thread.join(2);self.log.close();self.closed=True
    def close(self):
        if self.closed:return
        self.proc.stdin.write('{"op":"close"}\n');self.proc.stdin.flush();self.proc.stdin.close();self.proc.wait(60);self.thread.join(2);self.log.close();self.closed=True
        assert self.proc.returncode==0,'Native supervised shutdown failed'

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);root=parser.parse_args().output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
    hosts=root/'hosts';hosts.mkdir(mode=0o700);workspace=root/'workspace';workspace.mkdir(mode=0o700)
    backend=Backend(hosts);provider=Provider(reasoning=True);leases=[];controllers=[];admitted=[];sandbox=None;report={'cases':[]}
    catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    capabilities=[{'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*(m['declaredThinkingEfforts'] or [])])),'tools':None} for m in catalog['models']]
    try:
        for _ in range(3):
            lease=socket.socket();lease.bind(('127.0.0.1',0));leases.append(lease)
        ports=[v.getsockname()[1] for v in leases];sandbox=Sandbox(root,[provider.port,*ports]);report['guard']=sandbox.verify(provider.port)
        report['runner']=sandbox.identity()
        config={'root':str(hosts),'database':str(root/'workflow.sqlite'),'authority':str(root/'data'),'backend':backend.origin}
        controller=Controller(config,root/'controller-1.log');controllers.append(controller)
        def terminal(seed,ident):
            return wait_until(lambda:next((v['event'] for v in controller.call('events',target=seed['run']['id']) if v['event'].get('type')=='prompt_result' and v['event'].get('id')==ident),None),40)
        def turn(seed,name,*actions):
            provider.enqueue(name,*actions);ident=uuid.uuid4().hex
            assert controller.call('request',target=seed['attempt']['id'],input={'op':'input','id':ident,'message':'Execute the scripted offline response.'})=={'received':True}
            result=terminal(seed,ident);assert result['status']=='completed',result;return result
        for i,(selector,effort) in enumerate([('aperture-openai/openai.gpt-6-sol','low'),('aperture-anthropic/anthropic.claude-sonnet-5','medium'),('aperture-google/gemini-3.8-flash','low')]):
            name=f'supervised-{i}';model=next(m for m in catalog['models'] if m['selector']==selector)
            role={'id':'fixture','name':'Fixture','category':'Test','purpose':'Offline lifecycle proof','instructions':'Return fixture markers.','enabled':True,'model':selector,'effort':effort};plan=rpc.policy(role,capabilities)
            seed=controller.call('seed',title=name,input={'name':'Coordinator','role':role,'model':plan,'coordinator':True,'input':'Offline lifecycle assignment','brief':{'criteria':[],'dependsOn':[]}});admitted.append(seed)
            attempt=seed['attempt']['id'];attempt_root=hosts/attempt
            definition={'id':model['id'],'name':model['id'],'reasoning':model['declaredReasoning'],'input':model['declaredInput'],'contextWindow':model['declaredContextWindow'],'maxTokens':model['declaredMaxOutputTokens']}
            if model['declaredThinkingMode']:definition['thinking']={'mode':model['declaredThinkingMode'],'efforts':model['declaredThinkingEfforts']}
            argv=sandbox.argv([str(sandbox.binary),'--mode','rpc','--no-ui','--no-tools','--no-extensions','--no-rules','--no-skills','--no-lsp','--no-title','--no-pty','--cwd',str(workspace),'--session-dir',str(attempt_root/'sessions'),'--model',selector,'--thinking','off','--system-prompt','Offline fixture. Use only the assigned host tool.'])
            env={k:os.environ[k] for k in ('PATH','HOME','LANG') if k in os.environ};env.update(PI_CONFIG_DIR=os.path.relpath(attempt_root/'omp-root',Path.home()),TMPDIR=str(attempt_root),NO_COLOR='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_CONFIG_NOSYSTEM='1')
            launch={'runnerCwd':str(workspace),'budgetRoot':str(root/'budgets'/seed['assignment']['id']),'providerToken':'synthetic-provider-token','model':plan,'port':ports[i],'command':argv[0],'args':argv[1:],'env':env,'coordinator':True,'recipients':['Coordinator'],'upstreamProvider':{'baseUrl':f'http://127.0.0.1:{provider.port}/{name}/{model["provider"]}/v1','api':model['api'],'apiKey':'synthetic-upstream-key','models':[definition]}}
            leases[i].close();identity=controller.call('launch',target=attempt,revision=seed['run']['controlRevision'],config=launch);seed['identity']=identity
            assert identity['hostPid']!=identity['ompPid'];assert controller.call('owned',target=attempt)['state']=='live'
            controller.call('activate',target=seed['run']['id']);before=len(provider.requests)
            turn(seed,name,{'kind':'text','text':'SUPERVISED_TEXT_'+str(i)})
            turn(seed,name,{'kind':'tool','name':'send_message','arguments':{'to':'Coordinator','body':'SUPERVISED_MESSAGE'}},{'kind':'text','text':'SUPERVISED_TOOL_COMPLETE'})
            assert len(provider.requests)-before==3 and backend.requests[-1]['attemptId']==attempt
            events=controller.call('events',target=seed['run']['id']);assert len([v for v in events if v['event'].get('type')=='provider_usage' and v['event'].get('state')=='actual'])==3
            assert 'synthetic-upstream-key' not in json.dumps(events) and 'synthetic-provider-token' not in json.dumps(events)
            pane=subprocess.run(['tmux','-S',str(hosts/'tmux.sock'),'capture-pane','-p','-t',identity['pane'],'-S','-2000'],capture_output=True,text=True,check=True).stdout
            assert 'SUPERVISED_TEXT_'+str(i) in pane and 'host ready' in pane,pane;(root/(name+'-pane.txt')).write_text(pane)
            report['cases'].append({'name':'native supervisor actual OMP text/tool/usage and visible tmux host','selector':selector,'identity':identity,'providerRequests':3,'status':'passed'})
            if i==0:
                controller.kill();controller=Controller(config,root/'controller-2.log');controllers.append(controller)
                assert controller.call('reattach',target=attempt)['reattached']
                current=controller.call('request',target=attempt,input={'op':'identity'})
                assert all(current[k]==identity[k] for k in ('hostPid','ompPid','sessionId'))
                turn(seed,name,{'kind':'text','text':'AFTER_NATIVE_CONTROLLER_RESTART'})
                report['cases'].append({'name':'actual controller SIGKILL and same OMP process/session reattachment with subsequent turn','identity':identity,'status':'passed'})
            assert controller.call('stop',target=seed['run']['id'])['stopped']
            assert controller.call('owned',target=attempt)['state']=='stopped'
        report['passed']=True
    except Exception as error:report['passed']=False;report['error']=str(error);traceback.print_exc()
    finally:
        for controller in controllers:
            try:controller.close()
            except Exception as error:report['passed']=False;report.setdefault('cleanupErrors',[]).append(str(error))
        states=[]
        for seed in admitted:
            for name in ('host-process','omp-process'):
                root_dir=hosts/seed['attempt']['id']/name
                result=subprocess.run([str(BIN/'crewroom-process'),'inspect',str(root_dir)],capture_output=True,text=True,timeout=15)
                state=json.loads(result.stdout);states.append(state['state'])
                if state['state']!='stopped':
                    report['passed']=False;report.setdefault('fallbackCleanup',[]).append(str(root_dir));subprocess.run([str(BIN/'crewroom-process'),'stop',str(root_dir)],capture_output=True,text=True,timeout=15)
        report['allOwnedProcessesExited']=all(s=='stopped' for s in states)
        for lease in leases:lease.close()
        provider.close();backend.close();report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors;report['backendRequests']=len(backend.requests)
        (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
        if sandbox and report['allOwnedProcessesExited']:sandbox.close()
        report['passed']=report.get('passed',False) and report['allOwnedProcessesExited'] and not provider.errors
        (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
