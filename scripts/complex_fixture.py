"""E2E-02: scripted specialists, real staged edits, unit/browser execution and publication."""
from pathlib import Path
from coding_fixture import tool, text, send, candidate, objects
from mailbox_contract_fixture import PROJECT

def enqueue_complex(provider):
    provider.enqueue('Coordinator',send('Guide','Plan the guide and empty-state contract.'),text('Awaiting guide plan.'),
        tool('claim_implementation',{'assignee':'SpecialistOne','instruction':'Implement the export, validation and guide in stages under this writer lease.'}),send('SpecialistOne','Implement all three accepted criteria.'),text('Implementation assigned.'),
        lambda r:tool('assign_review',{'assignee':'SpecialistTwo','candidateId':candidate(r),'instruction':'Run both required checks and independently review all criteria.'}),send('SpecialistTwo','Inspect the current candidate and run unit/browser checks.'),text('Validation assigned.'),
        send('Guide','Inspect the implemented GUIDE.md and confirm the accepted documentation contract.'),text('Guide assessment assigned.'),
        lambda r:tool('publish_change',{'candidateId':candidate(r),'summary':'Add saved-filter export with empty-state behavior, strict input validation and an updated user guide. Unit and browser checks passed on this exact candidate.'}),text('COMPLEX_COMPLETE'))
    for _ in range(10): provider.enqueue('Coordinator',text('COMPLEX_COMPLETE'))
    for path in ['filters.mjs','index.html','GUIDE.md']:
        content=(PROJECT/'docs/testing/fixtures/saved-filter-app'/path).read_text()
        def edit(request,path=path,content=content):
            prior=[item for item in objects(request['body']) if item.get('path')==path and 'content' in item and 'sha256' in item][-1]
            return tool('write_file',{'path':path,'content':content,'expectedHash':prior['sha256']})
        provider.enqueue('SpecialistOne',tool('repository_read',{'path':path}),edit)
    provider.enqueue('SpecialistOne',tool('finish_implementation',{'summary':'Implement saved-filter export, validation, empty state and user guide.'}),text('Implementation submitted.'))
    provider.enqueue('SpecialistTwo',tool('coding_status',{}),tool('repository_read',{'path':'filters.mjs'}),tool('repository_read',{'path':'index.html'}),tool('run_check',{'checkId':'unit'}),tool('run_check',{'checkId':'browser'}),
        lambda r:tool('submit_review',{'candidateId':candidate(r),'verdict':'pass','summary':'Independently inspected export, invalid/empty behavior, and guide against both executed checks.','findings':[],
            'coverage':[{'criterion':0,'assessment':'Valid exports round trip and empty export is [].','checkIds':['unit','browser']},{'criterion':1,'assessment':'Invalid input is rejected and stale output clears.','checkIds':['unit','browser']},{'criterion':2,'assessment':'The browser check reads the guide and verifies its empty/invalid contract.','checkIds':['browser']}]}),send('Coordinator','Both checks and independent review passed on this candidate.'),text('Review submitted.'))
    provider.enqueue('Guide',send('Coordinator','Document empty export as [] and explain required name/query fields and invalid input.'),text('Guide plan delivered.'),tool('repository_read',{'path':'GUIDE.md'}),send('Coordinator','The saved guide matches empty-state and input-validation behavior.'),text('Guide assessed.'))

def enqueue_drift(provider,root):
    import threading
    from omp_offline_fixture import wait_until
    def stage():
        wait_until(lambda:(root/'drift-ready').exists(),150)
        with provider.actions['Coordinator'].mutex: provider.actions['Coordinator'].queue.clear()
        def publish_when_ready(request):
            assessments=[item for item in objects(request['body']) if 'ready' in item and 'candidate' in item and 'blockers' in item]
            if assessments and assessments[-1]['ready'] and assessments[-1]['candidate'].get('refreshedFrom'):
                for _ in range(12):provider.enqueue('Coordinator',text('DRIFT_COMPLETE'))
                return tool('publish_change',{'candidateId':assessments[-1]['candidate']['id'],'summary':'Rebased saved-filter export onto the advanced target. Fresh unit/browser checks and independent review pass at this head/base.'})
            provider.enqueue('Coordinator',tool('coding_status',{}),publish_when_ready)
            return text('Awaiting fresh review and checks.')
        provider.enqueue('Coordinator',tool('coding_status',{}),lambda r:tool('assign_review',{'assignee':'SpecialistTwo','candidateId':candidate(r),'instruction':'Review the rebased candidate; previous check/review receipts are stale.'}),send('SpecialistTwo','Run fresh unit/browser checks and independent review for this rebased candidate.'),text('Drift validation assigned.'),tool('coding_status',{}),publish_when_ready)
        provider.enqueue('SpecialistTwo',tool('coding_status',{}),tool('repository_read',{'path':'filters.mjs'}),tool('run_check',{'checkId':'unit'}),tool('run_check',{'checkId':'browser'}),lambda r:tool('submit_review',{'candidateId':candidate(r),'verdict':'pass','summary':'Independently rechecked the rebased export and its current unit/browser evidence.','findings':[],'coverage':[{'criterion':i,'assessment':'Fresh checks and source inspection cover this criterion after target refresh.','checkIds':['unit','browser'] if i<2 else ['browser']} for i in range(3)]}),send('Coordinator','Fresh checks and independent review passed after target refresh.'),text('Drift review submitted.'))
        (root/'drift-scripted').write_text('ready')
    threading.Thread(target=stage,daemon=True).start()
