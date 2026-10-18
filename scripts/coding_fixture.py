"""Script only the external model boundary; production workflow executes every tool."""
import json
import threading

def objects(value):
    found = []
    if isinstance(value, dict):
        found.append(value)
        for item in value.values(): found.extend(objects(item))
    elif isinstance(value, list):
        for item in value: found.extend(objects(item))
    elif isinstance(value, str):
        try: parsed = json.loads(value)
        except (ValueError, TypeError): return found
        if isinstance(parsed, (dict, list)): found.extend(objects(parsed))
    return found

def tool(name, arguments): return {'kind':'tool', 'name':name, 'arguments':arguments}
def text(value): return {'kind':'text','text':value}
def send(to, body): return tool('send_message', {'to':to,'body':body})
def candidate(request): return [item for item in objects(request['body']) if 'implementationAttempts' in item and 'revision' in item][-1]['id']
def edit(content, chunk_size=None):
    return lambda request: {**tool('write_file', {'path':'sum.py','content':content, 'expectedHash':read_hash(request)}), 'chunkSize': chunk_size}
def read_hash(request): return [item for item in objects(request['body']) if item.get('path') == 'sum.py' and 'content' in item and 'sha256' in item][-1]['sha256']
def patch(old, new):
    return lambda request: tool('edit_file', {'path':'sum.py','expectedHash':read_hash(request),'replacements':[{'oldText':old,'newText':new}]})
def review(verdict):
    return lambda request: tool('submit_review', {'candidateId':candidate(request), 'verdict':verdict,
      'summary':'Current code and unit evidence inspected.',
      'findings': [] if verdict == 'pass' else [{'severity':'major','body':'Document the empty-list contract.'}],
      'coverage': [{'criterion':0,'assessment':'Unit test covers empty input.','checkIds':['unit']}, {'criterion':1,'assessment':'Unit test covers nonempty input.','checkIds':['unit']}, {'criterion':2,'assessment':'Function docstring explains the empty-list result.','checkIds':[]}] if verdict == 'pass' else []})

def inspect_check_output(request):
    receipts = [item for item in objects(request['body']) if item.get('checkId') == 'unit' and 'completedAt' in item]
    return tool('check_output', {'checkEvidenceId': receipts[-1]['id']})

def report_failed_check(request):
    outputs = [item for item in objects(request['body']) if item.get('checkId') == 'unit' and 'outputPreview' in item]
    if not outputs or '0 != 5' not in outputs[-1]['outputPreview']:
        return text('CODING_CHECK_OUTPUT_MISSING')
    return send('Coordinator', 'The actual unit check failed with 0 != 5. Repair the nonempty case.')

def enqueue_coding(provider, scenario, fault='none', root=None, delivery=False, learning=False):
    contents = ['def sum_values(xs):\n    """Return zero for an empty list."""\n    return sum(xs)\n']
    if scenario == 'rework': contents = ['def sum_values(xs):\n    return 0\n', 'def sum_values(xs):\n    return sum(xs)\n', *contents]
    if scenario == 'large': contents = ['# Retained source line\n' * 6000 + contents[0]]
    if delivery:
        provider.enqueue('Coordinator', send('Reporter', 'Report execution started using the configured In Progress transition.'))
        provider.enqueue('Reporter', tool('jira_report', {'kind':'in_progress','eventKey':'execution-started'}), text('PROGRESS_QUEUED'))
    provider.enqueue('Coordinator', tool('claim_implementation', {'assignee':'SpecialistOne','instruction':'Implement the accepted criteria.'}), send('SpecialistOne','Implement the criteria, then call finish_implementation.'), text('Implementation assigned.'))
    if fault in ('writer', 'pause'):
        entered = threading.Event(); release = threading.Event()
        provider.enqueue('SpecialistOne', tool('repository_read', {'path':'sum.py'}), edit(contents[0]), {'kind':'hold','entered':entered,'release':release})
        threading.Thread(target=lambda: (entered.wait(), (root/'coding-held').write_text('held')), daemon=True).start()
        if fault == 'pause':
            provider.enqueue('Coordinator', lambda request: tool('steering_receipt', {'messageId':[item['id'] for item in objects(request['body']) if item.get('body') == 'Preserve the sum contract and finish the independent review.' and item.get('delivery') in ('pending','dispatched','received')][-1], 'disposition':'applied', 'reason':'Retained the accepted criteria and review requirement.'}))
        provider.enqueue('Coordinator', text('Interrupted implementation retained; awaiting completion.'))
    if fault == 'shell':
        # R2 contract: an ad-hoc contained command with a detached child is running when the operator pauses.
        provider.enqueue('SpecialistOne', tool('run_shell', {'command': "(setsid sh -c 'while :; do echo child; sleep 0.2; done' &); i=0; while :; do echo tick $i; i=$((i+1)); sleep 0.05; done", 'timeoutMs': 600000}))
        provider.enqueue('Coordinator', text('Contained command interrupted; implementation continues.'))
    for index, content in enumerate(contents):
        if fault == 'shell' and index == 0:
            # R2: after the interrupted command, search the worktree and patch the exact span.
            provider.enqueue('SpecialistOne', tool('repository_search', {'query':'def sum_values'}), tool('repository_read', {'path':'sum.py'}), patch('def sum_values(xs): return 9\n', content), tool('finish_implementation', {'summary':f'Implement accepted criteria, round {index}'}))
        else: provider.enqueue('SpecialistOne', tool('repository_read', {'path':'sum.py'}), edit(content, 48 if scenario == 'large' else None), tool('finish_implementation', {'summary':f'Implement accepted criteria, round {index}'}))
        if fault in ('before-commit','after-commit','after-receipt'):
            entered = threading.Event(); release = threading.Event()
            provider.enqueue('SpecialistOne', {'kind':'hold','entered':entered,'release':release})
            threading.Thread(target=lambda: (entered.wait(), (root/'completion-held').write_text('held')), daemon=True).start()
        provider.enqueue('SpecialistOne', text('Implementation submitted.'))
        if fault == 'insert':
            provider.enqueue('Coordinator', tool('propose_team', {'specialists':[{'name':'SpecialistThree','roleId':'reviewer','input':'Inspect sum.py documentation without editing it, then report the assessment to Coordinator.'}]}), text('Documentation assessment requested.'), text('Waiting for the inserted specialist assessment.'))
            provider.enqueue('SpecialistThree', tool('repository_read', {'path':'sum.py'}), send('Coordinator','The empty-list contract is documented; continue independent validation.'), text('Documentation assessment delivered.'))
        provider.enqueue('Coordinator', lambda request: tool('assign_review', {'assignee':'SpecialistTwo','candidateId':candidate(request),'instruction':'Run required checks and independently review this exact candidate.'}), send('SpecialistTwo','Inspect coding_status, run the unit check and independently review the current candidate. Report the outcome.'), text('Validation assigned.'))
        provider.enqueue('SpecialistTwo', tool('coding_status', {}), tool('repository_read', {'path':'sum.py'}), tool('run_check', {'checkId':'unit'}))
        if scenario == 'rework' and index == 0:
            provider.enqueue('SpecialistTwo', inspect_check_output, report_failed_check, text('Check failure reported.'))
        else:
            provider.enqueue('SpecialistTwo', review('changes_requested' if scenario == 'rework' and index == 1 else 'pass'), send('Coordinator','The current check and review results are recorded.'), text('Review reported.'))
        provider.enqueue('Coordinator', tool('coding_status', {}))
        if index == len(contents)-1:
            if delivery: provider.enqueue('Coordinator', lambda request: tool('publish_change', {'candidateId':candidate(request), 'summary':'## Summary\n\nReturn zero for an empty list and document the contract.\n\n## Testing\n\nThe contained unit check passes on this candidate.'}))
            if learning:
                # R8: cite the recorded check that certified the candidate; the operator decides.
                provider.enqueue('Coordinator', lambda request: tool('propose_learning', {'target':{'kind':'role','roleId':'implementer'},
                    'change':'Document the empty-list contract in the function docstring before submitting.', 'rationale':'The accepted criteria required documentation that the first implementation omitted.',
                    'evidence':[{'kind':'check','id':[item for item in objects(request['body']) if item.get('checkId') == 'unit' and 'completedAt' in item][-1]['id']}]}))
            provider.enqueue('Coordinator', text('CODING_READY'))
        else: provider.enqueue('Coordinator', tool('claim_implementation', {'assignee':'SpecialistOne','instruction':'Repair the recorded check or independent review failure.'}), send('SpecialistOne','Repair the latest recorded failure, then call finish_implementation.'), text('Repair assigned.'))
    if delivery:
        reported = False
        def observe_delivery(request):
            nonlocal reported
            notices = sorted([item for item in objects(request['body']) if isinstance(item.get('payload'), dict) and 'disposition' in item and 'createdAt' in item], key=lambda item:item['createdAt'])
            publication = next((item['payload'] for item in reversed(notices) if item['payload'].get('kind') == 'publication'), {})
            jira = next((item['payload'] for item in reversed(notices) if item['payload'].get('kind') == 'jira'), {})
            if publication.get('prState') == 'confirmed' and not reported:
                reported = True
                provider.enqueue('Coordinator', text('PR_HANDOFF_REQUESTED'), observe_delivery)
                return send('Reporter', 'Inspect current coding evidence and post the verified PR link with event key pr-created.')
            provider.enqueue('Coordinator', observe_delivery)
            return text('DELIVERY_READY' if publication.get('ready') and jira.get('state') == 'confirmed' else 'DELIVERY_WAITING')
        provider.enqueue('Coordinator', observe_delivery)
        provider.enqueue('Reporter', tool('coding_status', {}), lambda request: tool('jira_report', {'kind':'pr_link','eventKey':'pr-created','candidateId':candidate(request)}), send('Coordinator','The verified PR link is queued in Jira; inspect the delivery receipt.'), text('REPORTER_COMPLETE'))
