#!/usr/bin/env python3
"""Real installed OMP through Rust RPC; synthetic model replies under the offline guard."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import traceback
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from omp_offline_fixture import Agent, Provider, Sandbox
PROJECT = Path(__file__).resolve().parents[2]
BIN = PROJECT / 'target/debug'

class NativeAgent(Agent):
    def __init__(self, *args, config_hook=None, **kwargs):
        def launch(argv, **options):
            temporary = Path(options['cwd']) / 'tmp'; temporary.mkdir(mode=0o700)
            options['env']['TMPDIR'] = str(temporary)
            if config_hook: config_hook(Path(options['env']['PI_CODING_AGENT_DIR']))
            spec = {'command': argv[0], 'args': argv[1:], 'cwd': str(options['cwd']), 'env': options['env']}
            child = subprocess.Popen([str(BIN / 'examples/rpc-bridge')], **options)
            child.stdin.write(json.dumps({'spec': spec, 'helper': str(BIN / 'crewroom-process')}) + '\n')
            child.stdin.flush()
            return child
        super().__init__(*args, **kwargs, process_factory=launch)
        try:
            self.native = self.wait_frame('native_ready', timeout=30)
            assert self.native['pid'] != self.proc.pid
            self.ownership = self.inspect()
            assert self.ownership['state'] == 'live', self.ownership
        except Exception:
            self.close()
            raise
    def inspect(self):
        value = subprocess.run([str(BIN / 'crewroom-process'), 'inspect', str(self.root / 'omp-process')], capture_output=True, text=True, timeout=10, check=True)
        return json.loads(value.stdout)
    def close(self, abrupt=False):
        if self.log.closed: return self.proc.returncode
        try:
            if self.proc.poll() is None:
                self.command('native_close', timeout=10)
                self.proc.stdin.close()
                self.proc.wait(timeout=10)
        finally:
            if self.proc.poll() is None: self.proc.kill(); self.proc.wait(timeout=5)
            stop = subprocess.run([str(BIN / 'crewroom-process'), 'stop', str(self.root / 'omp-process')], capture_output=True, text=True, timeout=10, check=True)
            self.stopped = json.loads(stop.stdout)
            super().close()
        assert self.stopped['state'] == 'stopped', self.stopped
        return self.proc.returncode

def policy(role, catalog, operation='resolve'):
    result = subprocess.run([str(BIN / 'examples/policy-oracle')], input=json.dumps([{'operation': operation, 'role': role, 'catalog': catalog, 'policy': {}, 'requirements': {}}]), capture_output=True, text=True, timeout=10, check=True)
    value = json.loads(result.stdout)[0]
    if 'error' in value: raise RuntimeError(value['error'])
    return value['value']
def settings(plan):
    result = subprocess.run([str(BIN / 'examples/policy-oracle')], input=json.dumps([{'operation': 'settings', 'plan':plan}]), capture_output=True, text=True, timeout=10, check=True)
    return json.loads(result.stdout)[0]['value']

def main():
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('--output',type=Path,required=True)
    root=parser.parse_args().output.resolve(); root.mkdir(parents=True,exist_ok=False,mode=0o700)
    catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    capabilities=[{'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':list(dict.fromkeys(['off',*(m['declaredThinkingEfforts'] or [])])),'tools':None} for m in catalog['models']]
    base={'id':'fixture','name':'Fixture','category':'Test','purpose':'Verify routing','instructions':'Return the fixture marker.','enabled':True}
    provider=Provider(); agents=[]; report={'cases':[]}; sandbox=None
    try:
        sandbox=Sandbox(root,[provider.port]); report['guard']=sandbox.verify(provider.port)
        report['runner']=sandbox.identity()
        for i,(selector,effort) in enumerate([('aperture-openai/openai.gpt-6-sol','low'),('aperture-anthropic/anthropic.claude-sonnet-5','medium'),('aperture-google/gemini-3.8-flash','low')]):
            plan=policy({**base,'model':selector,'effort':effort},capabilities)
            a=NativeAgent(f'policy-{i}',root,sandbox,provider,catalog,selector,settings=settings(plan)); agents.append(a)
            identity=a.command('native_configure',plan=plan)
            before=len(provider.requests); a.text_turn(provider,f'NATIVE_ADMITTED_{i}')
            requests=provider.requests[before:]; assert len(requests)==1 and requests[0]['provider']+'/'+requests[0]['model']==selector
            report['cases'].append({'name':'exact native preflight and model request','selector':selector,'effort':effort,'identity':identity,'ompPid':a.native['pid'],'bridgePid':a.proc.pid,'status':'passed'})
            if i==0:
                before=len(provider.requests);start=len(a.frames)
                provider.enqueue(a.name,{'kind':'error','status':429},{'kind':'text','text':'RECOVERED_SAME_MODEL'})
                a.command('prompt',message='Execute rate limited fixture.'); assert a.wait_frame('prompt_result',start)['status']=='completed'
                assert len(provider.requests)-before==2 and all(r['provider']+'/'+r['model']==selector for r in provider.requests[before:])
                report['cases'].append({'name':'429 bounded HTTP retry retains model','status':'passed'})
                before=len(provider.requests);start=len(a.frames)
                provider.enqueue(a.name,{'kind':'error','status':403})
                a.command('prompt',message='Execute denied fixture.'); a.wait_frame('prompt_result',start)
                assert len(provider.requests)-before==1
                assert any(m.get('stopReason')=='error' for m in a.command('get_messages')['messages'])
                report['cases'].append({'name':'403 exposes failure without fallback','status':'passed'})
                # Real output chunking for arguments plus plain large input tool result.
                payload='αβ🦀'*150000
                a.command('set_host_tools',tools=[{'name':'large_fixture','loadMode':'essential','description':'Echo a fixture payload.','parameters':{'type':'object','properties':{'content':{'type':'string'}},'required':['content']}}])
                start=len(a.frames);before=len(provider.requests)
                provider.enqueue(a.name,{'kind':'tool','name':'large_fixture','arguments':{'content':payload}},{'kind':'text','text':'LARGE_COMPLETE'})
                a.command('prompt',message='Execute the large tool fixture.')
                call=a.wait_frame('host_tool_call',start,timeout=30); assert call['arguments']=={'content':payload}
                a.command('native_send',frame={'type':'host_tool_result','id':call['id'],'result':{'content':[{'type':'text','text':payload}]}})
                assert a.wait_frame('prompt_result',start,timeout=30)['status']=='completed'
                messages=a.command('get_messages')['messages']
                result=next(m for m in messages if m.get('role')=='toolResult')
                truncation=result['details']['meta']['truncation']
                assert truncation['totalBytes']==len(payload.encode()) and truncation['elidedBytes']>0
                artifacts=list(a.session_dir.glob('*/'+truncation['artifactId']+'.large_fixture.log'))
                assert len(artifacts)==1 and artifacts[0].read_bytes()==payload.encode(), 'OMP did not retain the complete input result'
                request=provider.requests[-1]['body']
                output=next(v['output'] for v in request['input'] if v.get('type')=='function_call_output')
                assert output==result['content'][0]['text'] and 'artifact://' in output, 'Provider projection must match explicit OMP truncation'
                assert any(v.get('type')=='function_call' and json.loads(v['arguments'])=={'content':payload} for v in request['input'])
                report['cases'].append({'name':'actual OMP v2 arguments and input artifact retain 1.2 MB; provider result explicitly truncated','utf8Bytes':len(payload.encode()),'sha256':hashlib.sha256(payload.encode()).hexdigest(),'providerRequests':len(provider.requests)-before,'resultProjection':truncation,'status':'passed'})
            a.close()
        for selector,effort in [('aperture-openai/zai-org/GLM-5.2','medium'),('aperture-google/gemini-3.1-pro-preview','medium'),('aperture-bedrock-compat/us.anthropic.claude-haiku-4-5-20251001-v1:0','high')]:
            role={**base,'model':selector,'effort':effort}
            try: policy(role,capabilities); raise AssertionError('Unsupported effort admitted')
            except RuntimeError as error: assert 'Unsupported effort' in str(error)
            report['cases'].append({'name':'unsupported declared effort rejected before launch','selector':selector,'status':'passed'})
            stale=json.loads(json.dumps(capabilities))
            for m in stale:
                if m['provider']+'/'+m['id']==selector:m['efforts'].append(effort)
            plan=policy(role,stale);a=NativeAgent(f'stale-{len(agents)}',root,sandbox,provider,catalog,selector,settings=settings(plan));agents.append(a)
            before=len(provider.requests)
            try:a.command('native_configure',plan=plan);raise AssertionError('Stale effort admitted')
            except RuntimeError as error:assert 'Unsupported effort' in str(error),str(error)
            assert len(provider.requests)==before
            report['cases'].append({'name':'stale effort refused by real runner readback','selector':selector,'status':'passed'});a.close()
        report['passed']=True
    except Exception as error:
        report['passed']=False;report['error']=str(error);traceback.print_exc()
    finally:
        for a in agents:
            if not a.log.closed:a.close()
        provider.close(); report['allOwnedProcessesExited']=all(a.proc.poll() is not None and a.inspect()['state']=='stopped' for a in agents)
        report['providerRequests']=len(provider.requests);report['providerErrors']=provider.errors
        (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
        if sandbox and report['allOwnedProcessesExited']:sandbox.close()
        report['passed']=report.get('passed',False) and report['allOwnedProcessesExited'] and not provider.errors
        (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return 0 if report['passed'] else 1
if __name__=='__main__':raise SystemExit(main())
