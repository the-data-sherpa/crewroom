#!/usr/bin/env python3
"""Crewroom HTTP API -> production tmux hosts -> guarded OMP -> Rust Mail -> peers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import signal
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox

parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('--output', type=Path, required=True); parser.add_argument('--scenario', choices=['session','first-turn','cycles','steering','lost-receipt','adoption','refinement','refinement-browser','research','research-browser','scheduler','queue','cancel-continuation','budget-resume'], default='session')
options = parser.parse_args(); root = options.output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
repository = root / 'repository'; repository.mkdir()
subprocess.run(['git','init','-b','main',str(repository)], check=True, capture_output=True)
(repository / 'README.md').write_text('Design A: FIFO order\nDesign B: weighted priority\nTradeoff: starvation risk\n' if options.scenario in ['research','research-browser'] else 'Fixture repository\n')
subprocess.run(['git','-C',str(repository),'add','README.md'], check=True, capture_output=True)
subprocess.run(['git','-C',str(repository),'-c','user.name=Fixture','-c','user.email=fixture@example.test','commit','-m','Tracked fixture'], check=True, capture_output=True)
commit = subprocess.run(['git','-C',str(repository),'rev-parse','HEAD'], check=True, capture_output=True, text=True).stdout.strip()
provider = Provider(); mail_port = free_port(); server_port = free_port(); host_ports = [free_port() for _ in range(4)]
if options.scenario in ['research', 'research-browser']:
    jira_ledger = []; jira_comments = []
    class JiraHandler(BaseHTTPRequestHandler):
        def log_message(self, *_args): pass
        def do_GET(self):
            jira_ledger.append(('GET', self.path))
            body = {'comments': jira_comments, 'total': len(jira_comments), 'startAt': 0, 'maxResults': 100}
            self.send_response(200); self.send_header('Content-Type', 'application/json'); self.end_headers(); self.wfile.write(json.dumps(body).encode())
        def do_POST(self):
            jira_ledger.append(('POST', self.path))
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if not self.path.endswith('/comment'):
                self.send_error(403); return
            jira_comments.append({'id': 'fixture-comment-1', 'body': body['body']})
            self.send_response(201); self.send_header('Content-Type', 'application/json'); self.end_headers(); self.wfile.write(b'{"id":"fixture-comment-1"}')
    jira_http = ThreadingHTTPServer(('127.0.0.1', 0), JiraHandler)
    jira_thread = threading.Thread(target=jira_http.serve_forever, daemon=True); jira_thread.start()
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
if options.scenario == 'budget-resume':
    providers['aperture-openai']['models'][0]['contextWindow'] = 200000
role_path = root / 'roles.json'
role_path.write_text(json.dumps([{'id': ident, 'name': name, 'category': 'Fixture', 'purpose': 'Message exchange', 'instructions': 'Use only the assigned external message tool.', 'model': selector, 'effort': effort, 'enabled': True} for ident, name, selector, effort in zip(['triage','implementer','reviewer'],names,selectors,efforts)] + ([{'id': ident, 'name': name, 'category': 'Research', 'purpose': 'Prepare cited non-coding evidence.', 'instructions': 'Only submit a cited research artifact.', 'model': selector, 'effort': effort, 'enabled': True} for ident, name, selector, effort in [('principal-architect','Principal architect',selectors[1],efforts[1]),('tech-writer','Technical writer',selectors[2],efforts[2])]] if options.scenario in ['research', 'research-browser'] else [])))
runtime = {'root': str(root / 'hosts'), 'socket': str(root / 'crewroom.sock'), 'ompBinary': str(sandbox.binary),
           'launchPrefix': sandbox.argv([]), 'hostPorts': host_ports,
           'mail': {'url': f'http://127.0.0.1:{mail_port}/mcp/', 'token': mail.token, 'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True},
           'catalog': capabilities, 'policy': {}, 'models': {'providers': providers}}
config = root / 'driver.json'; config.write_text(json.dumps({'serverPort': server_port, 'root': str(root), 'repository': str(root / 'repository'), 'commit': commit if options.scenario in ['research','research-browser'] else None, 'jiraApiUrl': f'http://127.0.0.1:{jira_http.server_port}' if options.scenario in ['research','research-browser'] else None, 'rolePath': str(role_path), 'runtime': runtime, 'scenario': options.scenario})); config.chmod(0o600)
if options.scenario == 'refinement-browser':
    provider.enqueue('Coordinator', {'kind':'text','text':'CODING_ACTIVE'},
        {'kind':'tool','name':'propose_refinement','arguments':{'brief':'Explore proposal','criteria':['Explore criterion'],'decisions':['Explore decision'],'unresolvedQuestions':['Explore question?']}},
        {'kind':'text','text':'EXPLORE_PROPOSED'},
        {'kind':'tool','name':'propose_refinement','arguments':{'brief':'Grill challenge','criteria':['Grill criterion'],'decisions':['Grill decision'],'unresolvedQuestions':['Grill question?']}},
        {'kind':'text','text':'GRILL_PROPOSED'})
elif options.scenario == 'refinement':
    def refinement_receipt(_request):
        deadline = time.monotonic() + 10
        while not (root/'refinement-steering.json').exists() and time.monotonic() < deadline: time.sleep(.01)
        message = json.loads((root/'refinement-steering.json').read_text())
        return {'kind':'tool','name':'steering_receipt','arguments':{'messageId':message['id'],'disposition':'applied','reason':'Included the zero case in the proposed criteria.'}}
    provider.enqueue('Coordinator', {'kind':'text','text':'CODING_ACTIVE'},
        {'kind':'tool','name':'propose_refinement','arguments':{'brief':'Explore proposal','criteria':['Explore criterion'],'decisions':['Explore decision'],'unresolvedQuestions':['Explore question?']}},
        {'kind':'text','text':'EXPLORE_PROPOSED'},
        refinement_receipt,
        {'kind':'text','text':'REFINEMENT_STEERING_RECONCILED'},
        {'kind':'tool','name':'propose_refinement','arguments':{'brief':'Grill challenge','criteria':['Grill criterion'],'decisions':['Grill decision'],'unresolvedQuestions':['Grill question?']}},
        {'kind':'text','text':'GRILL_PROPOSED'})
elif options.scenario in ['research', 'research-browser']:
    citation = {'path':'README.md', 'commit':commit, 'startLine':1, 'endLine':2, 'excerpt':'Design A: FIFO order\nDesign B: weighted priority'}
    comparison = {'title':'Queue design assessment', 'kind':'design', 'body':'# Comparison\nFIFO is predictable; priority may starve lower-ranked work.', 'citations':[citation], 'limitations':'Fixture-only comparison; no production workloads.', 'coverage':[{'criterionId':0,'section':'Comparison'}]}
    draft = {'title':'Decision draft', 'kind':'documentation-draft', 'body':'# Decision criteria\nFavor fairness when starvation is unacceptable.', 'citations':[citation], 'limitations':'Draft document, not a repository commit.', 'coverage':[{'criterionId':1,'section':'Decision criteria'}]}
    provider.enqueue('Coordinator',
        {'kind':'tool','name':'propose_team','arguments':{'specialists':[{'name':'Architect','roleId':'principal-architect','input':'Compare the two queue designs.','criteria':[{'index':0,'obligation':'Compare FIFO and priority from pinned source.'}]},{'name':'TechWriter','roleId':'tech-writer','input':'Draft decision criteria.','criteria':[{'index':1,'obligation':'Draft a decision artifact with limitations.'}]}]}},
        {'kind':'text','text':'TEAM_PROPOSED'}, {'kind':'text','text':'TEAM_READY'},
        {'kind':'tool','name':'request_research_completion','arguments':{}}, {'kind':'text','text':'INCOMPLETE_HANDOFF'},
        {'kind':'tool','name':'send_message','arguments':{'to':'Architect','body':'Correct your unsupported source citation and resubmit.'}}, {'kind':'text','text':'CORRECTION_REQUESTED'},
        {'kind':'tool','name':'request_research_completion','arguments':{}},
        *([{'kind':'tool','name':'jira_report','arguments':{'kind':'research_outcome','eventKey':'research-outcome-3'}}] if options.scenario == 'research' else []),
        {'kind':'text','text':'OUTCOME_REQUESTED'})
    provider.enqueue('Architect', {'kind':'tool','name':'submit_research_artifact','arguments':{**comparison,'citations':[{**citation,'path':'missing.md'}]}},
        {'kind':'text','text':'BAD_HANDOFF'}, {'kind':'tool','name':'submit_research_artifact','arguments':comparison}, {'kind':'text','text':'CORRECTED_HANDOFF'})
    provider.enqueue('TechWriter', {'kind':'tool','name':'submit_research_artifact','arguments':draft}, {'kind':'text','text':'DRAFT_READY'})
elif options.scenario == 'cancel-continuation':
    provider.enqueue('Coordinator', {'kind':'text','text':'CANCEL_CHECKPOINT'}, {'kind':'text','text':'LINKED_SUCCESSOR_STARTED'})
elif options.scenario == 'budget-resume':
    provider.enqueue('Coordinator', {'kind':'text','text':'CHECKPOINT_COMPLETE' + hashlib.shake_256(b'oversized-history').hexdigest(40000)}, {'kind':'text','text':'CONTINUED_FROM_CHECKPOINT'})
elif options.scenario in ['session', 'lost-receipt']:
    provider.enqueue('Coordinator', {'kind':'text','text':'CHECKPOINT_COMPLETE'}, {'kind':'text','text':'CONTINUED_FROM_CHECKPOINT'})
elif options.scenario == 'steering':
    entered = threading.Event(); release = threading.Event()
    receipt = {'kind':'tool','name':'steering_receipt','arguments':{'messageId':'pending','disposition':'applied','reason':'Added documentation to the plan.'}}
    provider.enqueue('Coordinator', {'kind':'hold','entered':entered,'release':release}, receipt, {'kind':'text','text':'STEERING_RECONCILED'})
    def unlock():
        entered.wait(); (root/'held-0').write_text('held')
        while not (root/'steering.json').exists(): time.sleep(0.02)
        receipt['arguments']['messageId'] = json.loads((root/'steering.json').read_text())['id']
        release.set()
    threading.Thread(target=unlock, daemon=True).start()
elif options.scenario == 'scheduler':
    provider.enqueue('Coordinator', {'kind':'text','text':'FIRST_EXECUTING'}, {'kind':'text','text':'SECOND_EXECUTING'})
elif options.scenario == 'queue':
    def queue_turn(request):
        instructions = request['body']['instructions']
        for run in 'ABC':
            if f'Assignment: Run {run}' in instructions:
                return {'kind':'text','text':f'QUEUE_{run}_EXECUTING'}
        raise AssertionError('Unexpected queue coordinator assignment')
    provider.enqueue('Coordinator', *([queue_turn] * 12))
elif options.scenario == 'adoption':
    receipt = {'kind':'tool','name':'steering_receipt','arguments':{'messageId':'pending','disposition':'applied','reason':'Re-briefing after the accepted change.'}}
    provider.enqueue('Coordinator', {'kind':'text','text':'ADOPTION_CHECKPOINT'}, receipt, {'kind':'text','text':'ADOPTED_REQUIREMENTS_SEEN'})
    def unlock_adoption():
        while not (root/'adoption.json').exists(): time.sleep(0.02)
        receipt['arguments']['messageId'] = json.loads((root/'adoption.json').read_text())['id']
        (root/'adoption-ready').write_text('ready')
    threading.Thread(target=unlock_adoption, daemon=True).start()
else:
    for index in range(3 if options.scenario == 'cycles' else 1):
        entered = threading.Event(); release = threading.Event()
        provider.enqueue('Coordinator', {'kind':'hold', 'entered':entered, 'release':release})
        threading.Thread(target=lambda event=entered, n=index: (event.wait(), (root/f'held-{n}').write_text('held')), daemon=True).start()
    if options.scenario == 'first-turn': provider.enqueue('Coordinator', {'kind':'text','text':'CONTINUED_FROM_CHECKPOINT'})
result = None
try:
    guard = sandbox.verify(provider.port); mail.start()
    if options.scenario in ['refinement-browser','research-browser']:
        runtime_path = root / 'runtime.json'; runtime_path.write_text(json.dumps(runtime))
        with (root / 'server.log').open('w') as server_log:
            process = subprocess.Popen(['node','--import','tsx','server/index.ts','--production'], cwd=PROJECT,
                env={**os.environ, 'CREWROOM_PORT':str(server_port), 'CREWROOM_DATA_DIR':str(root),
                     'CREWROOM_ROLE_PATH':str(role_path), 'CREWROOM_RUNTIME_CONFIG':str(runtime_path)}, stdout=server_log, stderr=subprocess.STDOUT)
            try:
                import urllib.request
                origin = f'http://127.0.0.1:{server_port}'
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    if process.poll() is not None: raise RuntimeError('Browser fixture server exited before ready.')
                    try:
                        with urllib.request.urlopen(origin + '/api/workspace', timeout=1) as response:
                            if response.status == 200: break
                    except Exception: time.sleep(.05)
                else: raise RuntimeError('Browser fixture server did not become ready.')
                (root / 'ready.json').write_text(json.dumps({'origin': origin}))
                deadline = time.monotonic() + 95
                while not (root / 'stop').exists() and time.monotonic() < deadline and process.poll() is None: time.sleep(.05)
                if not (root / 'stop').exists(): raise RuntimeError('Browser fixture did not complete normally.')
            finally:
                process.send_signal(signal.SIGTERM)
                result = subprocess.CompletedProcess(process.args, process.wait(timeout=15), '', '')
    else:
        result = subprocess.run(['node','--import','tsx','scripts/receipt-restart-driver.ts' if options.scenario == 'lost-receipt' else 'scripts/continuity-driver.ts',str(config)], cwd=PROJECT, capture_output=True,text=True,timeout=140 if options.scenario == 'queue' else 100 if options.scenario in ['research','research-browser','scheduler'] else 70)
        (root/'result.txt').write_text(result.stdout+result.stderr)
        if result.returncode: print(result.stdout+result.stderr,end='')
finally:
    mail.stop(); provider.close()
    if options.scenario in ['research', 'research-browser']:
        jira_http.shutdown(); jira_http.server_close(); jira_thread.join(2)
        (root/'jira-ledger.json').write_text(json.dumps({'requests':jira_ledger,'comments':jira_comments},indent=2))
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
    context_preserved = options.scenario != 'session' or (len(provider.requests) == 2 and all(text in json.dumps(provider.requests[1]['body']) for text in ['CHECKPOINT_COMPLETE', 'Document the fixture.']))
    expected_requests = (16 if options.scenario == 'research' else 15 if options.scenario == 'research-browser' else 7 if options.scenario == 'refinement' else 5 if options.scenario == 'refinement-browser' else 3 if options.scenario in ['cycles','steering','adoption'] else 2)
    request_count_ok = len(provider.requests) >= 3 if options.scenario == 'queue' else len(provider.requests) == expected_requests
    passed = result is not None and result.returncode == 0 and request_count_ok and not provider.errors and not surviving and context_preserved
    if options.scenario == 'budget-resume':
        passed = passed and 'CHECKPOINT_COMPLETE' not in json.dumps(provider.requests[-1]['body']) and 'Document the fixture.' in json.dumps(provider.requests[-1]['body'])
    if options.scenario == 'adoption':
        passed = passed and all(text in json.dumps(provider.requests[1]['body']) for text in ['ADOPTED_SOURCE_CRITERION', 'Changed after coordinator checkpoint.'])
    if options.scenario == 'research':
        passed = passed and len(jira_comments) == 1 and all(not path.endswith('/transitions') for _, path in jira_ledger)
    report = {'passed':passed,'guard':guard,'contextPreserved':context_preserved,'providerRequests':len(provider.requests),'providerErrors':provider.errors,'unexpectedSurvivingProcesses':surviving}
    if result and result.returncode == 0 and options.scenario not in ['refinement-browser','research-browser']: report['runtime'] = json.loads(result.stdout.strip().splitlines()[-1])
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    if not surviving: sandbox.close()
    print(json.dumps({key:value for key,value in report.items() if key!='runtime'}))
raise SystemExit(0 if passed else 1)
