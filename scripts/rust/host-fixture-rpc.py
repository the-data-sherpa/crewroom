#!/usr/bin/env python3
"""Independent scripted RPC peer for native host fault tests; never a production runner."""
import base64,hashlib,json,os,sys,threading,time
from pathlib import Path
root=Path.cwd(); settings=json.loads((root/'fixture-settings.json').read_text())
def session_name(name,directory):
    return name+'-'+hashlib.sha256(str(directory.resolve()).encode()).hexdigest()[:16] if settings.get('uniqueSessions') else name
lock=threading.Lock(); pending={}; streaming=False; directory=root/'sessions'; directory.mkdir(exist_ok=True); session=session_name('fixture-session',directory)
def emit(value):
    raw=json.dumps(value,ensure_ascii=False,separators=(',',':')).encode()
    with lock:
        if len(raw)+1<=1048576:print(raw.decode(),flush=True)
        else:
            count=(len(raw)+262143)//262144;chunk_id='fixture-'+str(time.time_ns())
            for i in range(count):print(json.dumps({'type':'rpc_chunk','chunkId':chunk_id,'index':i,'count':count,'byteLength':len(raw),'data':base64.b64encode(raw[i*262144:(i+1)*262144]).decode()}),flush=True)
def reply(frame,value=None):emit({'type':'response','id':frame['id'],'command':frame['type'],'success':True,'data':value})
def gate(name):
    (root/(name+'-waiting')).write_text('waiting')
    deadline=time.monotonic()+20
    while not (root/(name+'-release')).exists() and time.monotonic()<deadline:time.sleep(.005)
def finish(ident,status='completed'):
    global streaming
    streaming=False;emit({'type':'prompt_result','id':ident,'status':status,'agentInvoked':True,'sessionSettled':True})
def prompt(frame):
    global streaming
    try: action=json.loads(frame['message'])
    except ValueError:action={}
    if action.get('kind')=='exit':sys.exit(17)
    streaming=True
    if (root/'prompt-wait').exists():gate('prompt')
    if action.get('kind')=='terminal-first':finish(frame['id']);gate('prompt-reply');reply(frame);return
    if action.get('kind')=='unknown':return
    reply(frame)
    if action.get('kind')=='tool':
        ident=action.get('id','tool-wire');pending[ident]=frame['id']
        emit({'type':'host_tool_call','id':ident,'toolCallId':action.get('toolCallId','tool-call'),'toolName':action['name'],'arguments':action.get('arguments',{})})
        for _ in range(action.get('duplicates',0)):emit({'type':'host_tool_call','id':ident,'toolCallId':action.get('toolCallId','tool-call'),'toolName':action['name'],'arguments':action.get('arguments',{})})
        if action.get('conflict'):emit({'type':'host_tool_call','id':ident,'toolCallId':'changed-call','toolName':action['name'],'arguments':{}})
        if action.get('cancelGate'):
            def cancel():gate(action['cancelGate']);emit({'type':'host_tool_cancel','targetId':ident})
            threading.Thread(target=cancel,daemon=True).start()
        return
    for event in action.get('events',[]):emit(event)
    if action.get('fixtureEvents'):
        for event in json.loads((root/'fixture-events.json').read_text()):emit(event)
    if action.get('gate'):
        def later():gate(action['gate']);finish(frame['id'])
        threading.Thread(target=later,daemon=True).start()
    else:finish(frame['id'])
for line in sys.stdin:
    frame=json.loads(line)
    with (root/'rpc-requests.jsonl').open('a') as out:out.write(json.dumps(frame)+'\n');out.flush()
    kind=frame['type']
    if kind=='negotiate_protocol':
        if settings.get('setupGate'):gate(settings['setupGate'])
        reply(frame,{'protocolVersion':settings.get('protocolVersion',2)})
    elif kind=='get_available_models':reply(frame,{'models':[{'provider':'fixture','id':'model','api':'openai-responses'}]})
    elif kind=='get_available_thinking_levels':reply(frame,{'levels':['low']})
    elif kind=='get_state':
        if (root/'state-wait').exists():gate('state')
        reply(frame,{'model':{'provider':'fixture','id':'model','api':'openai-responses'},'thinkingLevel':'low','sessionId':session,'sessionFile':str(directory/'session.jsonl'),'isStreaming':streaming or (root/'busy').exists(),'isCompacting':None if (root/'malformed-state').exists() else (root/'compacting').exists(),'autoCompactionEnabled':False,'headers':{'secret':'never expose'},'token':'fixture-hidden'})
    elif kind in ['set_model','set_thinking_level','set_auto_compaction','set_auto_retry']:reply(frame)
    elif kind=='set_host_tools':reply(frame,{'toolNames':[v['name'] for v in frame['tools']]})
    elif kind=='get_messages_page':reply(frame,{'messages':[{'role':'user','content':'restored'}] if session=='saved-session' else [],'totalMessages':1 if session=='saved-session' else 0})
    elif kind=='open_session':
        directory=Path(frame['sessionDir']);resumed=(directory/'saved-session').exists();session=session_name('saved-session' if resumed else 'fresh-session',directory);reply(frame,{'cancelled':False,'resumed':resumed,'sessionId':session,'sessionFile':str(directory/'session.jsonl')})
    elif kind=='prompt':prompt(frame)
    elif kind=='host_tool_result':
        if frame['id'] in pending:finish(pending.pop(frame['id']),'error' if frame.get('isError') else 'completed')
    elif kind=='abort':
        streaming=False
        for ident,original in list(pending.items()):emit({'type':'host_tool_cancel','targetId':ident});finish(original,'aborted')
        pending.clear();reply(frame)
    else:reply(frame)
