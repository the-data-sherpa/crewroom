#!/usr/bin/env python3
"""Crewroom HTTP API -> production tmux hosts -> guarded OMP -> Rust Mail -> peers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import signal
from docker_engine import docker_socket, python_check_image
from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox

parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--fault', choices=['pause','cancel','host','tmux','supervisor','supervisor-host','detached','budget','complete'], default='pause')
arguments = parser.parse_args()
root = arguments.output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
(root/'workspace').mkdir()
provider = Provider(); mail_port = free_port(); server_port = free_port(); host_ports = [free_port() for _ in range(4)]
sandbox = Sandbox(root, [provider.port, mail_port, server_port, *host_ports], [mail_port, *host_ports])
mail = RustMail(root / 'mail', sandbox, mail_port)
catalog = json.loads((PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
names = ['Coordinator', 'SpecialistOne', 'SpecialistTwo']
selectors = ['aperture-openai/openai.gpt-6-sol', 'aperture-anthropic/anthropic.claude-sonnet-5', 'aperture-google/gemini-3.8-flash']
efforts = ['low', 'medium', 'low']
capabilities = []; providers = {}
for selector in selectors:
    m = next(m for m in catalog['models'] if m['selector'] == selector)
    capabilities.append({'provider': m['provider'], 'id': m['id'], 'api': m['api'], 'inputs': m['declaredInput'], 'efforts': ['off', *m['declaredThinkingEfforts']], 'tools': True})
    providers[m['provider']] = {'api': m['api'], 'apiKey': 'synthetic-offline-key', 'baseUrl': f'http://127.0.0.1:{provider.port}/{{agent}}/{m["provider"]}/v1', 'models': [
        {'id': m['id'], 'name': m['id'], 'reasoning': m['declaredReasoning'], 'input': m['declaredInput'], 'contextWindow': m['declaredContextWindow'], 'maxTokens': m['declaredMaxOutputTokens'], 'thinking': {'mode': m['declaredThinkingMode'], 'efforts': m['declaredThinkingEfforts']}}]}
role_path = root / 'roles.json'
role_path.write_text(json.dumps([{'id': ident, 'name': name, 'category': 'Fixture', 'purpose': 'Message exchange', 'instructions': 'Use only the assigned external message tool.', 'model': selector, 'effort': effort, 'enabled': True} for ident, name, selector, effort in zip(['triage','implementer','reviewer'],names,selectors,efforts)]))
runtime = {'root': str(root / 'hosts'), 'socket': str(root / 'crewroom.sock'), 'ompBinary': str(sandbox.binary),
           'launchPrefix': ['/usr/bin/sandbox-exec', '-f', str(sandbox.path)], 'hostPorts': host_ports,
           'mail': {'url': f'http://127.0.0.1:{mail_port}/mcp/', 'token': mail.token, 'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True},
           'commandEngine': {'socket': docker_socket()},
           'tools': [{'id':'slow','image':python_check_image(),'command':'/bin/sh','args':['-c','while true; do echo tick >> /workspace/ticks; sleep 0.05; done'],'workspace':str(root/'workspace')}],
           'catalog': capabilities, 'policy': {}, 'models': {'providers': providers}}
if arguments.fault == 'detached':
    writer = 'import time; from pathlib import Path; p=Path("/workspace/ticks");\nwhile True:\n with p.open("a") as f: f.write("tick\\n"); f.flush()\n time.sleep(0.03)'
    runtime['tools'][0].update(command='/usr/local/bin/python3', args=['-c','import subprocess,time,sys; subprocess.Popen([sys.executable,"-c",sys.argv[1]],start_new_session=True,close_fds=True,env={}); time.sleep(60)', writer])
if arguments.fault == 'complete':
    runtime['tools'][0]['args'] = ['-c', 'echo tick >> /workspace/ticks; sleep 0.2; echo finished']
config = root / 'driver.json'; config.write_text(json.dumps({'serverPort': server_port, 'root': str(root), 'rolePath': str(role_path), 'runtime': runtime, 'fault': arguments.fault})); config.chmod(0o600)
provider.enqueue('Coordinator', {'kind':'tool','name':'run_command','arguments':{'commandId':'slow'}})
if arguments.fault == 'supervisor-host': provider.enqueue('Coordinator', {'kind':'text','text':'Recovered retained command state.'})
if arguments.fault == 'complete': provider.enqueue('Coordinator', {'kind':'text','text':'Command completed.'})
result = None; guard = None
try:
    guard = sandbox.verify(provider.port); mail.start()
    result = subprocess.run(['node','--import','tsx','scripts/interruption-driver.ts',str(config)],cwd=PROJECT,capture_output=True,text=True,timeout=90)
    (root/'result.txt').write_text(result.stdout+result.stderr)
    print(result.stdout+result.stderr,end='')
finally:
    mail.stop(); provider.close()
    subprocess.run(['tmux','-S',runtime['socket'],'kill-server'],capture_output=True)
    (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2))
    surviving = []
    # Fault cleanup is limited to commands carrying this unique disposable root.
    process_rows = subprocess.check_output(['ps','-axo','pid=,command='],text=True)
    for line in process_rows.splitlines():
        fields=line.strip().split(None,1)
        if len(fields)==2 and str(root) in fields[1] and int(fields[0])!=os.getpid():
            surviving.append(int(fields[0]))
            try: os.kill(int(fields[0]),signal.SIGKILL)
            except ProcessLookupError: pass
    containers = [json.loads(path.read_text()) for path in (root/'hosts').glob('*/tools/*/container.json')]
    passed = result is not None and result.returncode==0 and not provider.errors and not surviving and all(item.get('removed') for item in containers)
    report = {'passed':passed,'fault':arguments.fault,'guard':guard,'providerRequests':len(provider.requests),'providerErrors':provider.errors,'unexpectedSurvivingProcesses':surviving,'removedContainers':len(containers)}
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    sandbox.close()
    print(json.dumps(report))
raise SystemExit(0 if passed else 1)
