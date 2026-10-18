#!/usr/bin/env python3
"""Fresh CLI configuration, same-path whole-state restore, then a completed second run."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import time
from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox
from coding_fixture import enqueue_coding, tool, text, send

application_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=PROJECT,text=True).strip()
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--mail-binary',type=Path)
options=parser.parse_args();root=options.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
durable=root/'installation';durable.mkdir(mode=0o700);data=durable/'data';data.mkdir(mode=0o700)
control=root/'control';control.mkdir(mode=0o700);repository=durable/'repository';repository.mkdir()
def git(*args):return subprocess.check_output(['git','-C',str(repository),*args],text=True,stderr=subprocess.DEVNULL).strip()
git('init','-b','main');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid')
(repository/'sum.py').write_text('def sum_values(xs): return 9\n')
(repository/'test_sum.py').write_text('import unittest\nfrom sum import sum_values\nclass Test(unittest.TestCase):\n def test_empty(self): self.assertEqual(sum_values([]),0)\n def test_sum(self): self.assertEqual(sum_values([2,3]),5)\n')
(repository/'README.md').write_text('The sum contract includes empty input returning zero.\n')
(repository/'AGENTS.md').write_text('Implement the accepted sum task only. Run the configured unit check.\n')
git('add','.');git('commit','-m','Fresh installation fixture');head=git('rev-parse','HEAD')
provider=Provider();mail_port=free_port();server_port=free_port();host_ports=[free_port() for _ in range(4)]
sandbox=Sandbox(root,[provider.port,mail_port,server_port,*host_ports],[mail_port,*host_ports])
mail=RustMail(durable/'mail',sandbox,mail_port,binary=options.mail_binary)
catalog=json.loads((PROJECT/'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
selectors=['aperture-openai/openai.gpt-6-sol','aperture-anthropic/anthropic.claude-sonnet-5','aperture-google/gemini-3.8-flash'];efforts=['low','medium','low'];capabilities=[];providers={}
for selector in selectors:
 m=next(model for model in catalog['models'] if model['selector']==selector)
 capabilities.append({'provider':m['provider'],'id':m['id'],'api':m['api'],'inputs':m['declaredInput'],'efforts':['off',*m['declaredThinkingEfforts']],'tools':True})
 providers[m['provider']]={'api':m['api'],'apiKey':'synthetic-installation-key','baseUrl':f'http://127.0.0.1:{provider.port}/{{agent}}/{m["provider"]}/v1','models':[{'id':m['id'],'name':m['id'],'reasoning':m['declaredReasoning'],'input':m['declaredInput'],'contextWindow':m['declaredContextWindow'],'maxTokens':m['declaredMaxOutputTokens'],'thinking':{'mode':m['declaredThinkingMode'],'efforts':m['declaredThinkingEfforts']}}]}
runtime={'root':str(durable/'hosts'),'socket':str(root/'crewroom.sock'),'ompBinary':str(sandbox.binary),'launchPrefix':['/usr/bin/sandbox-exec','-f',str(sandbox.path)],'hostPorts':host_ports,
 'mail':{'url':f'http://127.0.0.1:{mail_port}/mcp/','token':mail.token,'projectRoot':mail.project,'retentionSeconds':86400,'atcDisabled':True},'commandEngine':{'socket':str(Path.home()/'.docker/run/docker.sock')},'catalog':capabilities,'policy':{},'models':{'providers':providers}}
roles=[[ident,selectors[index],efforts[index]] for ident,index in [('triage',0),('implementer',1),('reviewer',2),('tech-writer',1)]]
config=root/'driver.json';config.write_text(json.dumps({'root':str(data),'control':str(control),'repository':str(repository),'rolePath':str(durable/'user/roles.json'),'serverPort':server_port,'runtime':runtime,'roles':roles}));config.chmod(0o600)
enqueue_coding(provider,'simple')
provider.enqueue('ResearchCoordinator',send('Researcher','Read the pinned contract and submit a cited artifact.'),text('Awaiting research.'),tool('request_research_completion',{}),text('Research completion requested.'))
provider.enqueue('Researcher',tool('repository_read',{'path':'README.md'}),tool('submit_research_artifact',{'title':'Contract note','kind':'research','body':'# Contract\nThe documented sum contract includes zero for empty input.','citations':[{'path':'README.md','commit':head,'startLine':1,'endLine':1,'excerpt':'The sum contract includes empty input returning zero.'}],'limitations':'Scripted local fixture; no live provider delivery or model-quality claim.','coverage':[{'criterionId':0,'section':'Contract'}]}),send('ResearchCoordinator','Verified contract artifact is saved.'),text('Contract note submitted.'))

def inventory(path):
 rows={}
 for item in sorted(path.rglob('*')):
  mode=item.lstat().st_mode;value={'mode':stat.S_IMODE(mode)}
  if item.is_symlink():value.update(kind='symlink',target=os.readlink(item))
  elif item.is_file():value.update(kind='file',sha256=hashlib.sha256(item.read_bytes()).hexdigest(),bytes=item.stat().st_size)
  elif item.is_dir():value.update(kind='directory')
  else:raise RuntimeError(f'Unsupported live resource in stopped backup: {item}')
  rows[str(item.relative_to(path))]=value
 return rows

child=None;passed=False;error=None;backup_receipt=None
try:
 guard=sandbox.verify(provider.port);mail.start()
 with (root/'driver.log').open('w') as log:
  child=subprocess.Popen(['node','--import','tsx','scripts/installation-driver.ts',str(config)],cwd=PROJECT,env={**os.environ,'CREWROOM_JIRA_CONFIG':str(data/'jira.json'),'CREWROOM_PUBLICATION_CONFIG':str(data/'bitbucket.json'),'CREWROOM_BITBUCKET_TOKEN':''},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
  by=time.monotonic()+100
  while not (control/'backup-ready').exists() and child.poll() is None and time.monotonic()<by:time.sleep(.05)
  assert (control/'backup-ready').exists(),'Driver did not reach stopped backup boundary.'
  mail.stop()
  # Every mutable component is under this installation, including repository common Git metadata.
  before=inventory(durable);backup=root/'backup';shutil.copytree(durable,backup,symlinks=True)
  assert inventory(backup)==before,'Backup differs from stopped source inventory.'
  os.rename(durable,root/'preserved-before-restore')
  shutil.copytree(backup,durable,symlinks=True);assert inventory(durable)==before,'Restore changed file content, links or permissions.'
  backup_receipt={'originalPath':str(durable),'backup':str(backup),'preservedOriginal':str(root/'preserved-before-restore'),'entries':before,'samePath':True,'contentsLinksAndModesVerified':True}
  (root/'backup-inventory.json').write_text(json.dumps(backup_receipt,indent=2)+'\n');mail.start();(control/'restored').write_text('same paths restored')
  child.wait(timeout=100);assert child.returncode==0,'Driver failed; see driver.log.'
  result=json.loads((root/'driver.log').read_text().strip().splitlines()[-1]);assert result['passed'] is True
  passed=True
except BaseException as caught:
 error=f'{type(caught).__name__}: {caught}'
finally:
 if child and child.poll() is None:
  os.killpg(child.pid,signal.SIGINT)
  try:child.wait(timeout=15)
  except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait()
 mail.stop();provider.close()
 (root/'provider-requests.json').write_text(json.dumps(provider.requests,indent=2)+'\n')
 table=subprocess.check_output(['/bin/ps','-axo','pid=,command='],text=True)
 pids_path=control/'backend-pids.json';backend_pids=json.loads(pids_path.read_text()) if pids_path.exists() else []
 survivors=[line for line in table.splitlines() if (str(root) in line and any(term in line for term in ['host.ts','process-lease.py','/runtime/omp'])) or (line.strip().split()[0].isdigit() and int(line.strip().split()[0]) in backend_pids and 'server/index.ts' in line)]
 # Emergency teardown must not hide a failed application shutdown from the report.
 subprocess.run(['tmux','-S',runtime['socket'],'kill-server'],capture_output=True)
 containers=[json.loads(path.read_text()) for path in (durable/'hosts').glob('*/tools/*/container.json')]
 expected={name:capabilities[index] for name,index in [('Coordinator',0),('SpecialistOne',1),('SpecialistTwo',2),('ResearchCoordinator',0),('Researcher',1)]}
 observed=set(row['agent'] for row in provider.requests)==set(expected) and all(row['provider']==expected[row['agent']]['provider'] and row['model']==expected[row['agent']]['id'] for row in provider.requests)
 passed=passed and observed and not provider.errors and not survivors and all(row.get('removed') for row in containers)
 report={'passed':passed,'applicationCommit':application_commit,'error':error,'providerRequests':len(provider.requests),'providerErrors':provider.errors,'unexpectedSurvivingProcesses':survivors,'backupEntries':len(backup_receipt['entries']) if backup_receipt else 0,'mailRestarts':len(mail.generations),'retainedCommandReceipts':len(containers),'observedModelIdentities':observed,'runtime':result if passed else None,'scope':'Fresh configuration and whole-state same-path recovery with real CLI/OMP/Mail/Git/checks; scripted models; no live Jira or Bitbucket delivery.'}
 (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
 if not survivors:sandbox.close()
 print(json.dumps(report))
raise SystemExit(0 if passed else 1)
