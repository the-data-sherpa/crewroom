#!/usr/bin/env python3
"""Installed OMP through the catalog workflow with SCRIPTED provider replies.

Runs `verify_catalog_workflow.py` (native picker, production Rust daemon, real
external Rust Mail) against an actual OMP executable. The upstream provider is a
loopback fixture that answers each operator instruction deterministically; no
model service, operator credential or OMP login is used. This proves the launch,
host-tool and supervision contract of that OMP version, not live inference.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

PROJECT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT / 'scripts'))
from omp_offline_fixture import Provider  # noqa: E402

PROVIDER, MODEL = 'scripted-openai', 'scripted-sol'
REPLIES = {'send_message': 'SENT', 'submit_research_artifact': 'RECORDED',
           'request_research_completion': 'DONE'}


def text(item):
    return ''.join(p.get('text', '') for p in item.get('content', []) if isinstance(p, dict))


def reply(request):
    """One deterministic turn: the tool an instruction names, else a short reply."""
    items = request['body'].get('input', [])
    last = items[-1] if items else {}
    if last.get('type') == 'function_call_output':
        name = next(i['name'] for i in reversed(items)
                    if i.get('type') == 'function_call' and i.get('call_id') == last.get('call_id'))
        return {'kind': 'text', 'text': REPLIES.get(name, 'DONE')}
    message = next((text(i) for i in reversed(items) if i.get('role') == 'user'), '')
    tool = lambda name, **arguments: {'kind': 'tool', 'name': name,
                                      'arguments': {'i': 'Follow the operator instruction', **arguments}}
    found = re.search(r'Call send_message exactly once with to=("(?:[^"\\]|\\.)*") and body=("(?:[^"\\]|\\.)*")\.', message)
    if found:
        return tool('send_message', to=json.loads(found[1]), body=json.loads(found[2]))
    found = re.search(r'Call submit_research_artifact exactly once with this JSON: (\{.*\})\. Then reply RECORDED', message, re.S)
    if found:
        return tool('submit_research_artifact', **json.loads(found[1]))
    if 'Call request_research_completion exactly once' in message:
        return tool('request_research_completion')
    return {'kind': 'text', 'text': 'READY'}


class Script:
    def get_nowait(self):
        return reply


class Actions(dict):
    def get(self, agent, default=None):
        return Script()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--omp', type=Path, required=True)
    parser.add_argument('--mail', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    omp = args.omp.resolve()
    version = subprocess.run([str(omp), '--version'], capture_output=True, text=True, timeout=30, check=True).stdout.strip()
    provider = Provider(reasoning=True)
    provider.actions = Actions()
    definition = {'id': MODEL, 'name': MODEL, 'reasoning': True, 'input': ['text'],
                  'contextWindow': 1050000, 'maxTokens': 128000,
                  'thinking': {'mode': 'effort', 'efforts': ['low', 'medium']}}
    config = {'kind': 'omp', 'model': MODEL, 'omp': str(omp),
              'catalogProfile': {'adapter': 'omp', 'provider': PROVIDER, 'id': MODEL, 'api': 'openai-responses',
                                 'inputs': ['text'], 'efforts': ['low'], 'tools': True},
              'ompModels': {'providers': {PROVIDER: {
                  'baseUrl': f'http://127.0.0.1:{provider.port}/{{agent}}/{PROVIDER}/v1',
                  'api': 'openai-responses', 'apiKey': 'synthetic-offline-key', 'models': [definition]}}}}
    settings = root / 'scripted-provider.json'
    with open(settings, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as out:
        json.dump(config, out)
    report = {'inference': 'scripted', 'liveInference': False, 'ompVersion': version, 'scope': __doc__.splitlines()[0]}
    try:
        result = subprocess.run([sys.executable, str(PROJECT / 'scripts/verify_catalog_workflow.py'),
                                 '--configuration', str(settings), '--output', str(root / 'workflow'),
                                 '--mail', str(args.mail)], cwd=PROJECT)
        workflow = json.loads((root / 'workflow/workflow-result.json').read_text())
        report.update(exitCode=result.returncode, workflowPassed=workflow.get('passed'),
                      checks=workflow.get('checks'), error=workflow.get('error'))
        report['passed'] = result.returncode == 0 and workflow.get('passed') is True and not provider.errors
    except Exception as error:
        report.update(passed=False, error=str(error))
    finally:
        provider.close()
        agents = sorted({r['agent'] for r in provider.requests})
        report.update(providerRequests=len(provider.requests), providerAgents=agents, providerErrors=provider.errors)
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
