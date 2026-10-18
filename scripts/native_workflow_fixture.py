#!/usr/bin/env python3
"""Run opt-in live native harness acceptance with private real Rust Mail and native workflow."""
import argparse
import json
import os
from pathlib import Path
import subprocess
from mailbox_contract_fixture import RustMail, free_port


class MailGuard:
    def __init__(self, root, binary):
        self.root, self.binary = root, binary
    def argv(self, command):
        # No user home/config/credentials are visible to the disposable mail server.
        return ['bwrap', '--unshare-all', '--share-net', '--die-with-parent',
                '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin',
                '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib', '/lib64',
                '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
                '--bind', str(self.root), str(self.root),
                '--ro-bind', str(self.binary), str(self.binary),
                '--setenv', 'HOME', str(self.root), '--', *map(str, command)]


def main(kind):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    if kind == 'codex':
        parser.add_argument('--codex',type=Path,required=True)
        parser.add_argument('--auth-file',type=Path,required=True)
    else:
        parser.add_argument('--claude',type=Path,required=True)
        parser.add_argument('--native-home',type=Path,required=True)
        parser.add_argument('--resolved-model',default='claude-sonnet-5-5')
    parser.add_argument('--mail',type=Path,required=True)
    parser.add_argument('--model',default='gpt-6-astra' if kind == 'codex' else 'sonnet')
    args=parser.parse_args();root=args.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
    mail=RustMail(root/'mail',MailGuard(root,args.mail.resolve()),free_port(),binary=args.mail)
    try:
        mail.start()
        config={'root':str(root),'kind':kind,'model':args.model,
                'mail':{'url':f'http://127.0.0.1:{mail.port}/mcp/','token':mail.token,'projectRoot':mail.project,
                        'retentionSeconds':86400,'atcDisabled':True}}
        if kind == 'codex':
            config.update(codex=str(args.codex.resolve()),authFile=str(args.auth_file.resolve()))
        else:
            config.update(claude=str(args.claude.resolve()),nativeHome=str(args.native_home.resolve()),resolvedModel=args.resolved_model)
        path=root/'settings.json';path.write_text(json.dumps(config));path.chmod(0o600)
        env={**os.environ,'CREWROOM_RETAIN_FIXTURE':'1'}
        result=subprocess.run(['node','node_modules/tsx/dist/cli.mjs','scripts/rust/verify-native-workflow.ts',str(path)],env=env,timeout=540)
        raise SystemExit(result.returncode)
    finally:
        mail.stop()
        (root/'mail-identity.json').write_text(json.dumps(mail.identity(),indent=2)+'\n')
