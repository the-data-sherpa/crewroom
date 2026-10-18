#!/usr/bin/env python3
"""Crewroom HTTP API -> production tmux hosts -> guarded OMP -> Rust Mail -> peers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import signal
import threading
from docker_engine import docker_socket
from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox

parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('--output', type=Path, required=True); parser.add_argument('--fault', choices=['none','specialist','coordinator-send'], default='none'); parser.add_argument('--browser', action='store_true'); parser.add_argument('--repository', action='store_true'); parser.add_argument('--rotate', action='store_true'); parser.add_argument('--coding-fault', choices=['none','writer','pause','before-commit','after-commit','after-receipt','insert','shell'], default='none'); parser.add_argument('--coding', choices=['none','simple','rework','large','complex'], default='none'); parser.add_argument('--team-size', type=int, choices=[0,1,2,3], default=0); parser.add_argument('--team-fault', choices=['none','before-apply','after-apply'], default='none')
parser.add_argument('--operator-insertion', action='store_true', help='Exercise one operator proposal at a team restart boundary.')
parser.add_argument('--reasoning', action='store_true', help='Probe opaque reasoning replay through the installed OMP tool turns.')
parser.add_argument('--jira', action='store_true')
parser.add_argument('--delivery', action='store_true')
parser.add_argument('--concurrent', action='store_true', help='Two runs at once with pooled host ports; a second coding run is refused.')
parser.add_argument('--learning', action='store_true', help='The coordinator proposes a role update from recorded evidence; the operator approves it.')
parser.add_argument('--adversarial', action='store_true')
parser.add_argument('--target-drift', action='store_true')
options = parser.parse_args(); root = options.output.resolve(); root.mkdir(parents=True, exist_ok=bool(os.environ.get('CREWROOM_FIXTURE_PORTS')), mode=0o700)
if options.operator_insertion and (options.team_size != 1 or options.team_fault == 'none'):
    raise ValueError('Operator insertion requires team-size 1 and a team-fault boundary.')
if options.delivery:
    if options.jira or options.coding_fault != 'none': raise ValueError('Use the dedicated delivery flow without another fault driver.')
    if options.coding == 'none': options.coding = 'simple'
ports = json.loads(os.environ['CREWROOM_FIXTURE_PORTS']) if os.environ.get('CREWROOM_FIXTURE_PORTS') else None
provider = Provider(reasoning=options.reasoning,port=ports[0] if ports else 0); mail_port = ports[1] if ports else free_port(); server_port = ports[2] if ports else free_port(); host_ports = ports[4:] if ports else [free_port() for _ in range(8 if options.concurrent else 4)]
if options.adversarial:
    from offline_network import probe
    provider_probe = probe('fake-model-provider')
    (root/'provider-network.json').write_text(json.dumps(provider_probe))
sandbox = Sandbox(root, [provider.port, mail_port, server_port, *host_ports], [mail_port, *host_ports])
if ports: sandbox.argv = lambda command: list(map(str,command)) # macOS forbids nested sandbox_apply; inherit the outer guard.
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
if options.delivery:
    roles = json.loads(role_path.read_text()); roles.append({**roles[0], 'id':'delivery-reporter', 'name':'Reporter', 'instructions':'Report verified publication evidence through the Jira tool.'}); role_path.write_text(json.dumps(roles))
runtime = {'root': str(root / 'hosts'), 'socket': str(root / 'crewroom.sock'), 'ompBinary': str(sandbox.binary),
           'launchPrefix': [] if ports else ['/usr/bin/sandbox-exec', '-f', str(sandbox.path)], 'hostPorts': host_ports,
           'mail': {'url': f'http://127.0.0.1:{mail_port}/mcp/', 'token': mail.token, 'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True},
           'catalog': capabilities, 'policy': {}, 'models': {'providers': providers}}
if options.coding != 'none': runtime['commandEngine'] = {'socket': docker_socket()}
proxy = None
if options.fault == 'coordinator-send':
    from committed_send_proxy import CommittedSendProxy
    proxy = CommittedSendProxy(root, mail)
    runtime['mail']['url'] = f'http://127.0.0.1:{proxy.port}/mcp/'
config = root / 'driver.json'; config.write_text(json.dumps({'serverPort': server_port, 'root': str(root), 'rolePath': str(role_path), 'runtime': runtime, 'fault': options.fault, 'teamSize': options.team_size, 'rotate':options.rotate, 'teamFault':options.team_fault, 'operatorInsertion':options.operator_insertion, 'repository':options.repository, 'coding':options.coding, 'codingFault':options.coding_fault, 'delivery':options.delivery, 'learning':options.learning,'adversarial':options.adversarial,'targetDrift':options.target_drift,'providerPort':ports[3] if ports else 0,'allowedPorts':ports})); config.chmod(0o600)
def send(to, body): return {'kind': 'tool', 'name': 'send_message', 'arguments': {'to': to, 'body': body}}
if options.jira:
    roles = json.loads(role_path.read_text()); roles[1]['id'] = 'delivery-reporter'; role_path.write_text(json.dumps(roles))
    provider.enqueue('Coordinator', send('Reporter', 'Report execution progress.'), {'kind':'text','text':'REPORT_REQUESTED'}, {'kind':'text','text':'JIRA_REPORT_COMPLETE'}, {'kind':'text','text':'DELIVERY_NOTICE_OBSERVED'})
    provider.enqueue('Reporter', {'kind':'tool','name':'jira_updates','arguments':{}}, {'kind':'tool','name':'jira_report','arguments':{'kind':'milestone','eventKey':'investigation-started','body':'Investigation started.'}}, send('Coordinator', 'Jira update queued; delivery remains pending.'), {'kind':'text','text':'REPORTER_COMPLETE'})
elif options.fault == 'coordinator-send':
    provider.enqueue('Coordinator', send('SpecialistOne','COMMITTED_BEFORE_CRASH'), {'kind':'text','text':'CONTINUATION_OBSERVED'}, {'kind':'text','text':'SEND_RECOVERY_COMPLETE'})
    provider.enqueue('SpecialistOne', send('Coordinator','RECEIVED_ONCE'), {'kind':'text','text':'SPECIALIST_COMPLETE'})
elif options.coding == 'complex':
    from complex_fixture import enqueue_complex
    enqueue_complex(provider)
    if options.target_drift:
        from complex_fixture import enqueue_drift
        enqueue_drift(provider,root)
elif options.coding != 'none':
    from coding_fixture import enqueue_coding
    enqueue_coding(provider, options.coding, options.coding_fault, root, options.delivery, options.learning)
    if options.adversarial:
        from coding_fixture import tool, text
        provider.enqueue('OtherRunAgent',text('OTHER_RUN_IDLE'))
        for name,args in [('merge_pull_request',{}),('deploy_application',{}),('repository_read',{'path':'../private-secret'}),('write_file',{'path':'../foreign-sentinel','content':'forbidden','expectedHash':None}),('submit_review',{'candidateId':'00000000-0000-4000-8000-000000000000','verdict':'pass','summary':'Bypass review','coverage':[],'findings':[]})]:
            provider.enqueue('SpecialistTwo',tool(name,args))
        provider.enqueue('SpecialistTwo',tool('run_shell',{'command':"python3 -c 'import socket,errno;\ntry: socket.create_connection((\"192.0.2.1\",443),1)\nexcept OSError as e: assert e.errno in (1,13,101); print(\"CHILD_EGRESS_DENIED\")\nelse: raise AssertionError(\"Forbidden egress\")'",'timeoutMs':5000}),text('AUTHORITY_BOUNDARY_COMPLETE'))
elif options.concurrent:
    for coordinator, specialist, marker in (('Coordinator', 'SpecialistOne', 'A_COMPLETE'), ('CoordinatorB', 'SpecialistB', 'B_COMPLETE')):
        provider.enqueue(coordinator, send(specialist, 'Confirm your run.'), {'kind':'text','text':'Request sent.'}, {'kind':'text','text':marker})
        provider.enqueue(specialist, send(coordinator, 'Confirmed.'), {'kind':'text','text':'Reply sent.'})
elif options.team_size:
    team = [{'name':'SpecialistOne','roleId':'implementer','input':'Assess the implementation task and report to Coordinator.','criteria':[{'index':0,'obligation':'Confirm the configured specialist models are used.'}]},
            {'name':'SpecialistTwo','roleId':'custom-reviewer','input':'Assess performance and report to Coordinator.','criteria':[{'index':1,'obligation':'Verify assignment snapshots stay unchanged.'}],'dependsOn':['SpecialistOne']},
            {'name':'Reporter','roleId':'delivery-reporter','input':'Assess the reporting needs and report to Coordinator.'}][:options.team_size]
    if options.operator_insertion: team[0]['name'] = 'Specialist1'
    if options.team_fault != 'none': provider.enqueue('Coordinator', {'kind':'text','text':'CHECKPOINT_COMPLETE'}, {'kind':'text','text':'TEAM_INSERTED'})
    else: provider.enqueue('Coordinator', {'kind':'tool','name':'list_roles','arguments':{}}, {'kind':'tool','name':'propose_team','arguments':{'specialists':team}}, {'kind':'text','text':'TEAM_REQUESTED'}, {'kind':'text','text':'TEAM_INSERTED'})
    if options.operator_insertion: provider.enqueue('Coordinator', {'kind':'text','text':'INSERTION_NOTICE_OR_RESULT_RECEIVED'})
    for index, member in enumerate(team):
        provider.enqueue(member['name'], send('Coordinator',f'{member["name"]} assessment retained.'), {'kind':'text','text':'Assessment sent.'})
        provider.enqueue('Coordinator', {'kind':'text','text':'ALL_SPECIALISTS_RECONCILED' if index == len(team)-1 and not options.rotate else 'Specialist result received.'})
    if options.rotate:
        if options.team_size != 3 or options.team_fault != 'none': raise ValueError('Rotation requires the full initial team.')
        # The last original specialist's result triggers a safe replacement proposal.
        provider.actions['Coordinator'].queue.pop()
        provider.enqueue('Coordinator', {'kind':'tool','name':'propose_team','arguments':{'specialists':[{'name':'SpecialistThree','roleId':'implementer','input':'Assess the next phase and report to Coordinator.'}], 'retire':[{'name':'SpecialistOne','reason':'Assessment completed; next phase needs another specialist.'}]}}, {'kind':'text','text':'ROTATION_REQUESTED'}, {'kind':'text','text':'ROTATION_APPLIED'}, {'kind':'text','text':'ALL_SPECIALISTS_RECONCILED'})
        provider.enqueue('SpecialistThree', send('Coordinator','Next phase assessment retained.'), {'kind':'text','text':'Assessment sent.'})

else:
    provider.enqueue('Coordinator', send('SpecialistOne','Investigate zero.'), {'kind':'text','text':'First hop sent.'}, *([{'kind':'text','text':'Waiting for resumed specialist.'}] if options.fault != 'none' else []), {'kind':'text','text':'ROUNDTRIP_COMPLETE'})
    if options.fault != 'none':
        entered = threading.Event(); release = threading.Event()
        provider.enqueue('SpecialistOne', {'kind':'hold', 'entered':entered, 'release':release})
        threading.Thread(target=lambda: (entered.wait(), (root/'specialist-held').write_text('held')), daemon=True).start()
    provider.enqueue('SpecialistOne', send('SpecialistTwo','Please check zero.'), {'kind':'text','text':'Second hop sent.'}, send('Coordinator','Zero is confirmed.'), {'kind':'text','text':'Reply forwarded.'})
    provider.enqueue('SpecialistTwo', send('SpecialistOne','Zero passes.'), {'kind':'text','text':'Review sent.'})
    if options.browser: provider.enqueue('Coordinator', {'kind':'text','text':'DOCUMENTATION_PLANNED'}, {'kind':'text','text':'COORDINATOR_STARTED_FROM_BROWSER'})
result = None; browser_result = None; guard = None
try:
    guard = sandbox.verify(provider.port); mail.start()
    result = subprocess.run(['node','--import','tsx','scripts/jira-driver.ts' if options.jira else 'scripts/complex-driver.ts' if options.coding == 'complex' else 'scripts/coding-driver.ts' if options.coding != 'none' else 'scripts/concurrent-driver.ts' if options.concurrent else 'scripts/team-restart-driver.ts' if options.team_fault != 'none' else 'scripts/team-driver.ts' if options.team_size else 'scripts/supervision-driver.ts' if options.browser else 'scripts/send-recovery-driver.ts' if options.fault == 'coordinator-send' else 'scripts/runtime-driver.ts',str(config)], cwd=PROJECT, capture_output=True,text=True,timeout=180 if options.coding != 'none' else 70)
    (root/'result.txt').write_text(result.stdout+result.stderr)
    if result.returncode: print(result.stdout+result.stderr,end='')
    if result.returncode == 0 and options.coding != 'none' and options.browser and not options.delivery:
        browser_result = subprocess.run(['node','--import','tsx','scripts/results-driver.ts',str(config)], cwd=PROJECT, capture_output=True,text=True,timeout=90)
        (root/'browser-result.txt').write_text(browser_result.stdout+browser_result.stderr)
        if browser_result.returncode: print(browser_result.stdout+browser_result.stderr,end='')
finally:
    if proxy: proxy.close()
    mail.stop(); provider.close()
    subprocess.run(['tmux','-S',runtime['socket'],'kill-server'],capture_output=True)
    (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2))
    surviving = []
    for log in (root/'hosts').glob('*/events.jsonl'):
        for line in log.read_text().splitlines():
            event = json.loads(line)['event']
            if event.get('type') != 'host_ready': continue
            for pid in (event['hostPid'], event['ompPid']):
                probe = subprocess.run([os.environ.get('CREWROOM_PROCESS_INSPECTOR','/bin/ps'),'-o','command=','-p',str(pid)],capture_output=True,text=True)
                if probe.returncode == 0 and str(root) in probe.stdout:
                    surviving.append(pid)
                    # Exceptional cleanup only after checking this test's unique root in the command.
                    try: os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
    expected = range(25,100) if options.coding == 'complex' else range(25, 100) if options.delivery else (7, 8) if options.jira else [24 if options.coding_fault == 'insert' else 21 if options.coding_fault == 'shell' else 19 if options.coding_fault in ('before-commit','after-commit','after-receipt') else 23 if options.coding_fault == 'pause' else 22 if options.coding_fault == 'writer' else 52 if options.coding == 'rework' else 18 if options.coding in ('simple','large') else 10 if options.concurrent else 18 if options.rotate else 2 + 3 * options.team_size if options.team_fault != 'none' else 4 + 3 * options.team_size if options.team_size else 11 if options.browser else 5 if options.fault == 'coordinator-send' else 11 if options.fault != 'none' else 9]
    passed = result is not None and result.returncode == 0 and len(provider.requests) - (1 if options.learning else 0) - (1 if options.operator_insertion else 0) in expected and not provider.errors and not surviving and (browser_result is None or browser_result.returncode == 0)
    briefs_presented = None
    if options.team_size >= 2 and options.team_fault == 'none':
        # R6: the real runner's first request for a specialist carries its brief and its teammates' obligations.
        first = next((request for request in provider.requests if request['agent'] == 'SpecialistTwo'), None)
        text = json.dumps(first['body']) if first else ''
        briefs_presented = all(part in text for part in ('Verify assignment snapshots stay unchanged.', 'Confirm the configured specialist models are used.', 'SpecialistOne'))
        passed = passed and briefs_presented
    elif options.coding != 'none' and options.coding != 'complex':
        # R6: the reviewer's first real request carries the implementer's obligations and their checks.
        first = next((request for request in provider.requests if request['agent'] == 'SpecialistTwo'), None)
        text = json.dumps(first['body']) if first else ''
        briefs_presented = all(part in text for part in ('Make sum([]) return 0.', 'Independently verify this criterion on the exact candidate.', 'Empty input returns zero'))
        passed = passed and briefs_presented
    reasoning_replayed = None
    if options.reasoning:
        reasoning_replayed = {agent: any(request['agent'] == agent and marker in json.dumps(request['body']) for request in provider.requests)
            for agent, marker in [('Coordinator', 'fixture-opaque-'), ('SpecialistOne', 'fixture-opaque-'), ('SpecialistTwo', 'Zml4dHVyZS1vcGFxdWUt')]}
        # This installed OMP drops the synthetic Anthropic redacted-only block. Its gateway replay
        # boundary is covered directly in provider-gateway.test.ts; do not claim process replay here.
        passed = passed and reasoning_replayed['Coordinator'] and reasoning_replayed['SpecialistTwo']
    if proxy:
        sends = [row for row in proxy.committed if row['arguments'].get('body_md') == 'COMMITTED_BEFORE_CRASH']
        replies = [json.loads(json.loads(row['response'])['result']['content'][0]['text']) for row in sends]
        external_once = len(sends) == 2 and sends[0]['arguments']['idempotency_key'] == sends[1]['arguments']['idempotency_key'] and replies[1].get('idempotent_replay') is True and all(reply['count'] == 1 for reply in replies) and replies[0]['deliveries'][0]['payload']['id'] == replies[1]['deliveries'][0]['payload']['id']
        passed = passed and external_once and proxy.external_once
    observed_identities = None
    if options.coding == 'complex' and result and result.returncode == 0:
        runtime_result = json.loads(result.stdout.strip().splitlines()[-1])
        identities = {row['name']: row['model'] for row in runtime_result['identities']}
        observed_identities = set(identities) == {'Coordinator','SpecialistOne','SpecialistTwo','Guide'} and set(row['agent'] for row in provider.requests) == set(identities) and all(
            row['provider'] == identities[row['agent']]['provider'] and row['model'] == identities[row['agent']]['id'] for row in provider.requests)
        passed = passed and observed_identities
    report = {'passed':passed,'guard':guard,'providerRequests':len(provider.requests),'providerErrors':provider.errors,'unexpectedSurvivingProcesses':surviving,'briefsPresented':briefs_presented,'reasoningReplayed':reasoning_replayed,'observedIdentities':observed_identities}
    if browser_result and browser_result.returncode == 0: report['browser'] = json.loads(browser_result.stdout.strip().splitlines()[-1])
    if result and result.returncode == 0: report['runtime'] = json.loads(result.stdout.strip().splitlines()[-1])
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    if not surviving: sandbox.close()
    print(json.dumps({key:value for key,value in report.items() if key!='runtime'}))
raise SystemExit(0 if passed else 1)
