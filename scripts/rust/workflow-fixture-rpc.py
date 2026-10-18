#!/usr/bin/env python3
"""Scripted RPC with automatic private attempt setup for workflow integration."""
from pathlib import Path
import json,runpy,sys
root = Path.cwd()
if not (root / 'fixture-settings.json').exists():
    (root / 'fixture-settings.json').write_text('{}')
if '--unique-sessions' in sys.argv:
    settings=json.loads((root/'fixture-settings.json').read_text());settings['uniqueSessions']=True
    (root/'fixture-settings.json').write_text(json.dumps(settings))
runpy.run_path(str(Path(__file__).with_name('host-fixture-rpc.py')), run_name='__main__')
