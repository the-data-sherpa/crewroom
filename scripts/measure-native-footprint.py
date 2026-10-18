#!/usr/bin/env python3
"""Read-only RSS/CPU samples for explicitly selected local processes; no ownership claims."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import time

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--process',action='append',required=True,help='Label=PID; include each process exactly once.')
parser.add_argument('--seconds',type=int,default=10)
parser.add_argument('--bundle',type=Path)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
if not 1<=args.seconds<=30:parser.error('Choose 1 through 30 seconds.')
selected={}
for value in args.process:
 label,pid=value.rsplit('=',1)
 if not label or label in selected or not pid.isdigit() or int(pid)<=0:parser.error('Use unique labels and positive PIDs.')
 selected[label]=int(pid)
if len(set(selected.values()))!=len(selected):parser.error('Do not count a PID twice.')
if args.output.exists():parser.error('Output exists; preserve it and select another path.')

def inspect(pid):
 # comm is the executable name only; arguments may contain private credentials.
 def read(fields):return subprocess.check_output(['/bin/ps','-ww','-p',str(pid),'-o',fields],text=True).strip()
 started=read('lstart=');binary=read('comm=');rss=int(read('rss='));cpu=read('time=')
 days=0
 if '-' in cpu:
  day,cpu=cpu.split('-',1);days=int(day)
 parts=cpu.split(':');total=days*86400+sum(float(p)*60**i for i,p in enumerate(reversed(parts)))
 return {'started':started,'executable':binary,'rssKiB':rss,'cpuSeconds':total}

start=time.monotonic();samples=[];identities={}
while True:
 now=time.monotonic();rows={label:inspect(pid) for label,pid in selected.items()}
 for label,row in rows.items():
  identity=(row['started'],row['executable'])
  identities.setdefault(label,identity)
  if identities[label]!=identity:raise RuntimeError('A selected process changed identity; measurement is invalid.')
 samples.append({'elapsedSeconds':now-start,'processes':rows})
 if now-start>=args.seconds:break
 time.sleep(min(1,args.seconds-(now-start)))
elapsed=samples[-1]['elapsedSeconds']
summary={label:{'pid':pid,'executable':identities[label][1],'rssKiBMedian':statistics.median(s['processes'][label]['rssKiB'] for s in samples),'rssKiBMax':max(s['processes'][label]['rssKiB'] for s in samples),'cpuPercentDuringSample':100*(samples[-1]['processes'][label]['cpuSeconds']-samples[0]['processes'][label]['cpuSeconds'])/elapsed} for label,pid in selected.items()}
report={'sampledSeconds':elapsed,'processes':summary,'samples':samples,'scope':'Only explicitly listed PIDs. RSS sum double-counts shared pages; it is not PSS or total machine memory. CPU is observed cumulative time delta, not a responsiveness benchmark. Docker/VM, Mail and browser attribution must be stated separately.'}
if args.bundle:
 bundle=args.bundle.resolve();report['bundle']={'path':str(bundle),'fileBytes':sum(p.stat().st_size for p in bundle.rglob('*') if p.is_file()),'allocatedKiB':int(subprocess.check_output(['/usr/bin/du','-sk',str(bundle)],text=True).split()[0]),'binaries':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (bundle/'Contents/MacOS').iterdir() if p.is_file()}}
args.output.parent.mkdir(parents=True,exist_ok=True)
args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'output':str(args.output),'processes':summary}))
