#!/usr/bin/env python3
"""Independent native host + guarded installed OMP + synthetic provider responses."""
import argparse,hashlib,http.server,importlib.util,json,os,signal,socket,subprocess,sys,threading,traceback,urllib.error,urllib.request,uuid
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Provider,Sandbox,wait_until
module=importlib.util.spec_from_file_location('native_rpc',Path(__file__).with_name('verify-omp-rpc.py'));rpc=importlib.util.module_from_spec(module);module.loader.exec_module(rpc)
PROJECT=Path(__file__).resolve().parents[2];BIN=PROJECT/'target/debug'
class Backend:
    def __init__(self):
        self.requests=[];owner=self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                value=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                assert self.path=='/internal/agent-message'
                assert self.headers['Authorization']=='Bearer synthetic-controller-token'
                assert self.headers['Origin']==owner.origin
                owner.requests.append(value);self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"stored":true}')
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);self.origin=f'http://127.0.0.1:{self.server.server_port}'
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
    def close(self):self.server.shutdown();self.server.server_close();self.thread.join(2)
class TmuxProcess:
    """An exact owned host in a dedicated disposable tmux server, never the user's server."""
    def __init__(self,root,argv):
        self.root=root/'host-process';self.socket='crewroom-native-host-'+uuid.uuid4().hex;self.returncode=None;self.closed=False
        spec={'command':argv[0],'args':argv[1:],'cwd':str(root),'env':{k:os.environ[k] for k in ('PATH','HOME','LANG') if k in os.environ}}
        subprocess.run([str(BIN/'crewroom-process'),'prepare-owned',str(self.root)],input=json.dumps(spec),text=True,capture_output=True,check=True,timeout=10)
        self.tmux('new-session','-d','-s','fixture','-x','160','-y','40',str(BIN/'crewroom-process'),'exec',str(self.root));self.tmux('set-window-option','-t','fixture','remain-on-exit','on')
        wait_until(lambda:(self.root/'owner.json').exists());self.pid=json.loads((self.root/'owner.json').read_text())['pid']
    def tmux(self,*args):return subprocess.run(['tmux','-L',self.socket,'-f','/dev/null',*args],capture_output=True,text=True,check=True,timeout=10).stdout
    def poll(self):
        if self.returncode is not None:return self.returncode
        if self.tmux('display-message','-p','-t','fixture','#{pane_dead}').strip()=='1':self.returncode=int(self.tmux('display-message','-p','-t','fixture','#{pane_dead_status}').strip() or '1')
        return self.returncode
    def wait(self,timeout=10):wait_until(lambda:self.poll() is not None,timeout);return self.returncode
    def terminate(self):
        # The helper verifies PID/start/group identity before signalling anything.
        subprocess.run([str(BIN/'crewroom-process'),'stop',str(self.root)],capture_output=True,text=True,check=True,timeout=15)
    def kill(self):
        owner=json.loads(subprocess.run([str(BIN/'crewroom-process'),'inspect',str(self.root)],capture_output=True,text=True,check=True,timeout=10).stdout)
        assert owner['state']=='live',owner;os.kill(self.pid,signal.SIGKILL)
    def reconnect(self):
        client=subprocess.Popen(['tmux','-L',self.socket,'-C','attach-session','-t','fixture'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        output,error=client.communicate('detach-client\n',timeout=10);assert client.returncode==0,(output,error);assert '%exit' in output,output;return output
    def finish(self):
        if self.closed:return
        stopped=json.loads(subprocess.run([str(BIN/'crewroom-process'),'stop',str(self.root)],capture_output=True,text=True,check=True,timeout=15).stdout);assert stopped['state']=='stopped',stopped
        self.tmux('kill-server');self.closed=True

class Host:
    def __init__(self,name,root,sandbox,provider,backend,model,plan,port,tmux=False,runner_cwd=None,**extra):
        self.name=name;self.root=root/name;self.root.mkdir(mode=0o700);self.origin=f'http://127.0.0.1:{port}'
        definition={'id':model['id'],'name':model['id'],'reasoning':model['declaredReasoning'],'input':model['declaredInput'],'contextWindow':model['declaredContextWindow'],'maxTokens':model['declaredMaxOutputTokens']}
        if model['declaredThinkingMode']:definition['thinking']={'mode':model['declaredThinkingMode'],'efforts':model['declaredThinkingEfforts']}
        argv=sandbox.argv([str(sandbox.binary),'--mode','rpc','--no-ui','--no-tools','--no-extensions','--no-rules','--no-skills','--no-lsp','--no-title','--no-pty','--cwd',str(runner_cwd or root/'workspace'),'--session-dir',str(self.root/'sessions'),'--model',model['selector'],'--thinking','off','--system-prompt','Offline fixture. Use only the assigned host tool.'])
        env={k:os.environ[k] for k in ('PATH','HOME','LANG') if k in os.environ};env.update(PI_CONFIG_DIR=os.path.relpath(self.root/'omp-root',Path.home()),NO_COLOR='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_CONFIG_NOSYSTEM='1')
        config={'root':str(self.root),'helper':str(BIN/'crewroom-process'),'token':'synthetic-controller-token','providerToken':'synthetic-provider-token','backend':backend.origin,'attemptId':name,'model':plan,'port':port,'upstreamProvider':{'baseUrl':f'http://127.0.0.1:{provider.port}/{name}/{model["provider"]}/v1','api':model['api'],'apiKey':'synthetic-upstream-key','models':[definition]},'command':argv[0],'args':argv[1:],'env':env,'recipients':['Coordinator'],**extra}
        path=self.root/'host.json';path.write_text(json.dumps(config));path.chmod(0o600)
        self.out=(self.root/'pane.log').open('w');self.err=(self.root/'stderr.log').open('w');self.proc=TmuxProcess(self.root,[str(BIN/'crewroom-host'),str(path)]) if tmux else subprocess.Popen([str(BIN/'crewroom-host'),str(path)],stdout=self.out,stderr=self.err)
    def ready(self):
        def read():
            if self.proc.poll() is not None:raise RuntimeError(f'Host failed: {self.root}: '+(self.root/'stderr.log').read_text()+json.dumps(self.events()))
            return (self.root/'endpoint.json').exists()
        wait_until(read,30);self.identity=self.control('identity');assert self.identity['hostPid']==self.proc.pid and self.identity['ompPid']!=self.proc.pid
        assert self.ownership()['state']=='live';return self.identity
    def control(self,op,**body):
        req=urllib.request.Request(self.origin,data=json.dumps({'op':op,**body}).encode(),headers={'Authorization':'Bearer synthetic-controller-token','Content-Type':'application/json'})
        with urllib.request.urlopen(req,timeout=25) as response:return json.load(response)
    def events(self):
        path=self.root/'events.jsonl'
        if not path.exists():return []
        lines=path.read_bytes().splitlines(keepends=True)
        return [json.loads(line)['event'] for line in lines if line.endswith(b'\n')]
    def terminal(self,ident):return wait_until(lambda:next((v for v in self.events() if v.get('type')=='prompt_result' and v.get('id')==ident),None),40)
    def turn(self,provider,*actions):
        provider.enqueue(self.name,*actions);ident=uuid.uuid4().hex
        assert self.control('input',id=ident,message='Execute the scripted offline response.')=={'received':True}
        result=self.terminal(ident);assert self.control('input_status',id=ident)['disposition']==result['status'];return result
    def ownership(self):return json.loads(subprocess.run([str(BIN/'crewroom-process'),'inspect',str(self.root/'omp-process')],capture_output=True,text=True,check=True,timeout=10).stdout)
    def close(self):
        if self.out.closed:return
        try:
            if self.proc.poll() is None:
                try:self.control('close')
                except Exception:self.proc.terminate()
                try:self.proc.wait(timeout=10)
                except subprocess.TimeoutExpired:self.proc.kill();self.proc.wait(timeout=5)
        finally:
            stopped=json.loads(subprocess.run([str(BIN/'crewroom-process'),'stop',str(self.root/'omp-process')],capture_output=True,text=True,check=True,timeout=15).stdout)
            self.out.close();self.err.close();assert stopped['state']=='stopped',stopped
            if isinstance(self.proc,TmuxProcess):self.proc.finish()

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);root=parser.parse_args().output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
    (root/'workspace').mkdir(mode=0o700)
    catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text());capabilities=[{'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*(m['declaredThinkingEfforts'] or [])])),'tools':None} for m in catalog['models']]
    base={'id':'fixture','name':'Fixture','category':'Test','purpose':'Host integration','instructions':'Return fixture markers.','enabled':True}
    leases=[]
    for _ in range(11):
        s=socket.socket();s.bind(('127.0.0.1',0));leases.append(s)
    ports=[s.getsockname()[1] for s in leases];provider=Provider(reasoning=True);backend=Backend();hosts=[];sandbox=None;report={'cases':[]}
    try:
        # The guard must permit an actually listening loopback endpoint for its positive probe.
        sandbox=Sandbox(root,[provider.port,*ports]);report['guard']=sandbox.verify(provider.port)
        report['runner']=sandbox.identity()
        def create(name,selector='aperture-openai/openai.gpt-6-sol',effort='low',ready=True,**extra):
            model=next(m for m in catalog['models'] if m['selector']==selector);plan=rpc.policy({**base,'model':selector,'effort':effort},capabilities);i=len(hosts);leases[i].close()
            host=Host(name,root,sandbox,provider,backend,model,plan,ports[i],**extra);hosts.append(host)
            if ready:host.ready()
            return host
        original=None;assignment_budget=root/'budgets'/'family-0'
        for i,(selector,effort) in enumerate([('aperture-openai/openai.gpt-6-sol','low'),('aperture-anthropic/anthropic.claude-sonnet-5','medium'),('aperture-google/gemini-3.8-flash','low')]):
            # Production keys the opaque-reasoning budget by assignment, shared by its attempts
            # (`budgetRoot` in supervisor configuration). The session-restore cases below reuse it.
            h=create(f'family-{i}',selector,effort,**({'budgetRoot':str(assignment_budget)} if i==0 else {}));before=len(provider.requests)
            assert h.turn(provider,{'kind':'text','text':'NATIVE_HOST_TEXT'})['status']=='completed'
            assert h.turn(provider,{'kind':'tool','name':'send_message','arguments':{'to':'Coordinator','body':'NATIVE_HOST_MESSAGE'}},{'kind':'text','text':'NATIVE_HOST_TOOL_COMPLETE'})['status']=='completed'
            assert len(provider.requests)-before==3
            assert len([v for v in h.events() if v['type']=='provider_usage' and v['state']=='actual'])==3
            assert backend.requests[-1]['attemptId']==h.name
            assert not any(key in json.dumps(h.events()) for key in ['synthetic-upstream-key','synthetic-provider-token','synthetic-controller-token'])
            report['cases'].append({'name':'independent host text and routed tool with reported synthetic usage','selector':selector,'identity':h.identity,'providerRequests':3,'status':'passed'})
            h.close()
            if i==0:original=h
        for name,expected in [('restore',original.identity['sessionId']),('wrong-id','wrong-id')]:
            h=create(name,previousAttemptId=original.name,previousSessionId=expected,resumeSessionDir=original.identity['sessionDirectory'],budgetRoot=str(assignment_budget));sent=len(provider.requests)
            restored=name=='restore';assert h.identity['recoveryMode']==('session' if restored else 'replacement'),h.identity
            assert h.identity['sessionDirectory']==(original.identity['sessionDirectory'] if restored else str(h.root/'sessions'))
            assert (h.identity['sessionId']==original.identity['sessionId'])==restored
            assert h.turn(provider,{'kind':'text','text':'AFTER_RESTORE'})['status']=='completed'
            assert Path(h.control('state')['sessionFile']).parent==Path(h.identity['sessionDirectory'])
            replayed=[i for r in provider.requests[sent:] if r['agent']==name for i in r['body'].get('input',[]) if i.get('type')=='reasoning']
            report['cases'].append({'name':name+' actual session directory and subsequent turn','identity':h.identity,'replayedOpaqueReasoning':len(replayed),'status':'passed'});h.close()
        empty=root/'empty-sessions';empty.mkdir(mode=0o700)
        h=create('empty-history',previousAttemptId='empty',previousSessionId='empty',resumeSessionDir=str(empty));assert h.identity['recoveryMode']=='replacement' and h.identity['sessionDirectory']==str(h.root/'sessions');h.close();report['cases'].append({'name':'empty history uses new attempt session directory','status':'passed'})
        for name,action in [('denied',{'kind':'error','status':403}),('effort',{'kind':'error','status':400,'error':{'type':'invalid_request_error','param':'reasoning.effort','message':"Unsupported reasoning_effort 'low'. Supported values are 'medium'."}})]:
            h=create(name);before=len(provider.requests);result=h.turn(provider,action);assert result['status']=='error',result;assert len(provider.requests)-before==1
            report['cases'].append({'name':name+' exposes terminal error without upstream substitution','terminal':result,'providerRequests':1,'status':'passed'});h.close()
        moved=root/'moved-workspace';moved.mkdir(mode=0o700);before=len(provider.requests)
        h=create('cancelled-restore',runner_cwd=moved,ready=False,previousAttemptId=original.name,previousSessionId=original.identity['sessionId'],resumeSessionDir=original.identity['sessionDirectory'])
        try:h.ready();raise AssertionError('Cancelled restore became ready')
        except RuntimeError as error:assert 'Session restore was not confirmed.' in str(error)
        assert not (h.root/'endpoint.json').exists() and len(provider.requests)==before;h.close()
        report['cases'].append({'name':'runner rejects changed working directory; cancelled restore never becomes ready','status':'passed'})
        h=create('tmux',tmux=True);identity=h.identity
        assert h.turn(provider,{'kind':'text','text':'TMUX_BEFORE_RECONNECT'})['status']=='completed';first=h.proc.reconnect();second=h.proc.reconnect()
        assert h.control('identity')==identity
        assert h.turn(provider,{'kind':'text','text':'TMUX_AFTER_RECONNECT'})['status']=='completed'
        pane=h.proc.tmux('capture-pane','-p','-t','fixture','-S','-2000');assert 'TMUX_BEFORE_RECONNECT' in pane and 'TMUX_AFTER_RECONNECT' in pane and 'host ready' in pane,pane
        (h.root/'pane-capture.txt').write_text(pane);(h.root/'client-reconnect.txt').write_text(first+second)
        h.proc.kill();h.proc.wait();before_cleanup=h.ownership();h.close();assert h.ownership()['state']=='stopped'
        report['cases'].append({'name':'dedicated tmux reconnect, readable native host and external reconciliation after host death','identity':identity,'runnerOwnershipAfterHostDeath':before_cleanup,'status':'passed'})
        h=create('large');payload='αβ🦀'*150000;before=len(provider.requests)
        result=h.turn(provider,{'kind':'tool','name':'send_message','arguments':{'to':'Coordinator','body':payload}},{'kind':'text','text':'AFTER_LARGE_TOOL'})
        call=next(v for v in h.events() if v['type']=='host_tool_call');assert call['arguments']['body']==payload;assert backend.requests[-1]['arguments']['body']==payload
        pages=h.control('events',after=0);assert any(v['event'].get('type')=='host_tool_call' and v['event']['arguments']['body']==payload for v in pages)
        assert result['status'] in ('completed','error')
        if result['status']=='error':
            assert len(provider.requests)-before==1
            assert result['error']['message']=='409 Provider request exceeded its limit.' or any(v['type']=='model_policy_rejected' and v.get('code')=='input_budget' for v in h.events()),result
        report['cases'].append({'name':'actual OMP oversized host tool retained and routed in full','utf8Bytes':len(payload.encode()),'sha256':hashlib.sha256(payload.encode()).hexdigest(),'terminal':result,'providerRequests':len(provider.requests)-before,'status':'passed'});h.close()
        report['passed']=True
    except Exception as error:report['passed']=False;report['error']=str(error);traceback.print_exc()
    finally:
        for h in hosts:h.close()
        for s in leases:s.close()
        provider.close();backend.close();report['allOwnedProcessesExited']=all(h.proc.poll() is not None and h.ownership()['state']=='stopped' for h in hosts);report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors;report['backendRequests']=len(backend.requests)
        (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
        if sandbox and report['allOwnedProcessesExited']:sandbox.close()
        report['passed']=report.get('passed',False) and report['allOwnedProcessesExited'] and not provider.errors
        (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return 0 if report['passed'] else 1
if __name__=='__main__':raise SystemExit(main())
