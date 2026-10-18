#!/usr/bin/env python3
"""Bounded live tool-routing probe using the operator's existing OMP role selections."""
import argparse
import json
from pathlib import Path
import subprocess
import yaml
from mailbox_contract_fixture import PROJECT, RustMail, NetworkGuard, free_port

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--allow-live-models', action='store_true', required=True)
parser.add_argument('--output', type=Path, required=True)
options = parser.parse_args()
root = options.output.resolve(); root.mkdir(parents=True, mode=0o700)
settings = yaml.safe_load((Path.home()/'.omp/agent/config.yml').read_text())
definitions = yaml.safe_load((Path.home()/'.omp/agent/models.yml').read_text())
selection = [('Coordinator','coordinator','request_coordinator'),('Implementer','implementer','code'),('Reviewer','reviewer','review'),('Reporter','delivery-reporter','docs')]
roles, catalog, providers = [], [], {}
for name, role_id, source in selection:
    selector, effort = settings['modelRoles'][source].rsplit(':',1)
    provider, model_id = selector.split('/',1)
    definition = definitions['providers'][provider]
    model = next(item for item in definition['models'] if item['id']==model_id)
    if provider not in providers:
        providers[provider] = {**definition, 'models':[]}
        # The bounded admission probe verified this configured gateway requires Bearer auth.
        if provider == 'aperture-anthropic': providers[provider]['authMode'] = 'bearer'
    if not any(item['id']==model_id for item in providers[provider]['models']): providers[provider]['models'].append(model)
    if not any(item['provider']==provider and item['id']==model_id for item in catalog):
        catalog.append({'provider':provider,'id':model_id,'api':definition['api'],'inputs':model['input'],'efforts':['off',*model['thinking']['efforts']],'tools':True})
    roles.append({'id':role_id,'name':name,'category':'Admission probe','purpose':'Verify a bounded live message exchange','instructions':'This is a routing probe. Use only send_message for the explicitly named next recipient. Do not delegate, change models, request other tools or perform repository work. Stop after the requested handoff.','model':selector,'effort':effort,'enabled':True})
port=free_port(); mail_port=free_port(); host_ports=[free_port() for _ in range(4)]
mail=RustMail(root/'mail',NetworkGuard(root,[mail_port],[mail_port]),mail_port)
runtime={'root':str(root/'hosts'),'socket':str(root/'crewroom.sock'),'ompBinary':str(Path.home()/'.bun/bin/omp'),'hostPorts':host_ports,
         'mail':{'url':f'http://127.0.0.1:{mail_port}/mcp/','token':mail.token,'projectRoot':mail.project,'retentionSeconds':86400,'atcDisabled':True},
         'catalog':catalog,'policy':{},'models':{'providers':providers}}
role_path=root/'roles.json'; role_path.write_text(json.dumps(roles)); role_path.chmod(0o600)
config=root/'driver.json'; config.write_text(json.dumps({'root':str(root),'serverPort':port,'rolePath':str(role_path),'runtime':runtime}));config.chmod(0o600)
result = None
try:
    mail.start()
    result=subprocess.run(['node','--import','tsx','scripts/live-model-driver.ts',str(config)],cwd=PROJECT,capture_output=True,text=True,timeout=330)
    (root/'result.log').write_text(result.stdout+result.stderr)
except subprocess.TimeoutExpired:
    (root/'result.log').write_text('Live admission driver exceeded its absolute 330-second deadline.\n')
finally:
    # Use production process leases even if the bounded driver was terminated before its finally block.
    cleanup = None
    try:
        cleanup = subprocess.run(['node', '--import', 'tsx', '--input-type=module', '-e', '''
import { existsSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { stopOwned } from './server/runtime/processes.ts';
const root = process.argv[1], states = [];
if (existsSync(root)) for (const id of readdirSync(root)) {
  if (!existsSync(join(root, id, 'host.json'))) continue;
  for (const kind of ['host-process', 'omp-process']) states.push({ attemptId: id, kind, state: (await stopOwned(join(root, id, kind))).state });
}
console.log(JSON.stringify(states));
if (states.some(item => item.state !== 'stopped')) process.exitCode = 1;
''', str(root/'hosts')], cwd=PROJECT, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        pass  # An uncertain lease must remain a failed cleanup, never a fabricated stopped process.
    finally:
        mail.stop()
        subprocess.run(['tmux','-S',runtime['socket'],'kill-server'],capture_output=True)
    report_path = root/'report.json'
    report = json.loads(report_path.read_text()) if report_path.exists() else {'passed':False,'reason':'Driver did not produce a completed admission report.'}
    report['cleanupConfirmed'] = cleanup is not None and cleanup.returncode == 0
    if report['cleanupConfirmed']: report['processOwnership'] = json.loads(cleanup.stdout)
    report['passed'] = report['passed'] and result is not None and result.returncode == 0 and report['cleanupConfirmed']
    report_path.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'passed':report['passed'],'report':str(report_path),'selections':[{k:r[k] for k in ['id','model','effort']} for r in roles]}))
raise SystemExit(0 if report['passed'] else 1)
