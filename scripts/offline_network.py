"""Fail-closed probes and observed listeners for the isolated authority scenario."""
import errno
import json
import os
from pathlib import Path
import socket
import subprocess
import sys

def probe(label):
    checks={}
    for name,host,port in [('external','192.0.2.1',443),('unallocated_loopback','127.0.0.1',1)]:
        try:
            with socket.create_connection((host,port),1): pass
            checks[name]=False
        except PermissionError: checks[name]=True
        except OSError: checks[name]=False
    if not all(checks.values()): raise RuntimeError(f'{label}: network guard ineffective: {checks}')
    return {'role':label,'pid':os.getpid(),**checks}

def listeners(root, allowed):
    rows=subprocess.check_output([os.environ.get('CREWROOM_PROCESS_INSPECTOR','/bin/ps'),'-Ao','pid=,ppid=,command='],text=True).splitlines()
    parsed=[(int(a),int(b),c) for line in rows for a,b,c in [line.strip().split(None,2)]]
    pids={pid for pid,parent,command in parsed if str(root) in command}
    for _ in range(20):
        expanded=pids|{pid for pid,parent,command in parsed if parent in pids}
        if expanded==pids: break
        pids=expanded
    result=subprocess.run(['lsof','-nP','-a','-p',','.join(map(str,sorted(pids))),'-iTCP','-sTCP:LISTEN','-Fpcn'],capture_output=True,text=True)
    if result.returncode not in (0,1): raise RuntimeError(result.stderr)
    ledger=[]; pid=None; command=None
    for row in result.stdout.splitlines():
        if row.startswith('p'): pid=int(row[1:])
        elif row.startswith('c'): command=row[1:]
        elif row.startswith('n'):
            address=row[1:]; host,port=address.rsplit(':',1)
            if host not in ['127.0.0.1','[::1]'] or int(port) not in allowed: raise RuntimeError(f'Uncontrolled listener {pid} {address}')
            ledger.append({'pid':pid,'command':command,'address':address})
    if not ledger: raise RuntimeError('No owned listener evidence was captured')
    udp=subprocess.run(['lsof','-nP','-a','-p',','.join(map(str,sorted(pids))),'-iUDP','-Fpcn'],capture_output=True,text=True)
    if udp.returncode not in (0,1) or any(row.startswith('n') for row in udp.stdout.splitlines()): raise RuntimeError('Uncontrolled UDP socket: '+udp.stdout)
    return ledger

if __name__=='__main__':
    root=Path(sys.argv[1]); ports=json.loads(sys.argv[2])
    print(json.dumps(listeners(root,ports)))
