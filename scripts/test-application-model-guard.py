#!/usr/bin/env python3
"""Crewroom HTTP API -> production tmux hosts -> guarded OMP -> Rust Mail -> peers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import signal
from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox

parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('--output', type=Path, required=True)
root = parser.parse_args().output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
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
           'catalog': capabilities, 'policy': {}, 'models': {'providers': providers}}
config = root / 'driver.json'; config.write_text(json.dumps({'serverPort': server_port, 'root': str(root), 'rolePath': str(role_path), 'runtime': runtime})); config.chmod(0o600)
provider.enqueue('Coordinator', {'kind':'error','status':400,'error':{'type':'invalid_request_error','param':'reasoning.effort','message':"Unsupported reasoning_effort 'low'. Supported values are 'medium'."}}, {'kind':'text','text':'UNAUTHORIZED_EFFORT_COMPLETED'})
result = None
try:
    guard = sandbox.verify(provider.port); mail.start()
    result = subprocess.run(['node','--import','tsx','scripts/model-guard-driver.ts',str(config)], cwd=PROJECT, capture_output=True,text=True,timeout=70)
    (root/'result.txt').write_text(result.stdout+result.stderr)
    if result.returncode: print(result.stdout+result.stderr,end='')
finally:
    mail.stop(); provider.close()
    subprocess.run(['tmux','-S',runtime['socket'],'kill-server'],capture_output=True)
    (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2))
    surviving = []
    for log in (root/'hosts').glob('*/events.jsonl'):
        for line in log.read_text().splitlines():
            event = json.loads(line)['event']
            if event.get('type') != 'host_ready': continue
            for pid in (event['hostPid'], event['ompPid']):
                probe = subprocess.run(['ps','-o','command=','-p',str(pid)],capture_output=True,text=True)
                if probe.returncode == 0 and str(root) in probe.stdout:
                    surviving.append(pid)
                    # Exceptional cleanup only after checking this test's unique root in the command.
                    try: os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
    passed = result is not None and result.returncode == 0 and len(provider.requests) == 1 and not provider.errors and not surviving
    report = {'passed':passed,'guard':guard,'providerRequests':len(provider.requests),'providerErrors':provider.errors,'unexpectedSurvivingProcesses':surviving}
    if result and result.returncode == 0: report['runtime'] = json.loads(result.stdout.strip().splitlines()[-1])
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    if not surviving: sandbox.close()
    print(json.dumps({key:value for key,value in report.items() if key!='runtime'}))
raise SystemExit(0 if passed else 1)
