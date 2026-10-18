"""Protocol, identity and failure tests; no installed harness or inference needed."""
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import discover_harness_models as discovery


class DiscoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="crewroom-discovery-test-")
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def executable(self, source):
        path = self.root / "harness"
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o700)
        return str(path)

    def test_codex_handshake_pagination_and_no_inference(self):
        log = self.root / "requests.jsonl"
        binary = self.executable(f'''
import json,sys
for line in sys.stdin:
 v=json.loads(line)
 with open({str(log)!r},'a') as f:f.write(line)
 if v['method']=='initialize': result={{}}
 elif v['method']=='initialized': continue
 elif v['method']=='model/list':
  second='cursor' in v['params']
  if second: assert v['params']['cursor']=='next'
  result={{'data':[{{'model':'two' if second else 'one','displayName':'Same label',
                    'inputModalities':['text'],'supportedReasoningEfforts':[{{'reasoningEffort':'ultra'}}],
                    'defaultReasoningEffort':'ultra','apiKey':'never-serialize'}}],
           'nextCursor':None if second else 'next'}}
 else: raise AssertionError('Unexpected inference or action')
 print(json.dumps({{'method':'notice','params':{{'private':'never-serialize'}}}}),flush=True)
 print(json.dumps({{'id':v['id'],'result':result}}),flush=True)
''')
        models = discovery.codex_models(binary, self.root, 2)
        self.assertEqual([v["selector"] for v in models], ["one", "two"])
        self.assertEqual(models[0]["reasoningEfforts"], ["ultra"])
        self.assertNotIn("never-serialize", json.dumps(models))
        frames = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual([v["method"] for v in frames],
                         ["initialize", "initialized", "model/list", "model/list"])
        self.assertEqual(models[0]["access"], "not_verified")

    def test_claude_only_initializes_and_preserves_aliases(self):
        binary = self.executable('''
import json,sys,os
assert '--bare' in sys.argv and '--no-session-persistence' in sys.argv
assert sys.argv[sys.argv.index('--tools')+1]==''
assert 'ANTHROPIC_API_KEY' not in os.environ
for line in sys.stdin:
 v=json.loads(line)
 assert v['type']=='control_request' and v['request']['subtype']=='initialize'
 models=[{'value':alias,'resolvedModel':'claude-example','displayName':'Example',
          'supportedEffortLevels':['low','high'],'secret':'never-serialize'} for alias in ['default','opus']]
 print(json.dumps({'type':'control_response','response':{'subtype':'success',
   'request_id':v['request_id'],'response':{'models':models,'account':'private-account'}}}),flush=True)
''')
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "never-inherit"}):
            models = discovery.claude_models(binary, self.root, 2)
        self.assertEqual([v["selector"] for v in models], ["default", "opus"])
        self.assertEqual([v["resolvedModel"] for v in models], ["claude-example"] * 2)
        self.assertNotIn("private-account", json.dumps(models))
        self.assertNotEqual(models[0]["key"], models[1]["key"])
        self.assertIsNone(models[0]["inputModalities"])

    def test_omp_whitelists_catalog_and_does_not_merge_providers(self):
        entries = [{"provider": provider, "id": "same", "selector": f"{provider}/same",
                    "name": "Same name", "thinking": None, "input": ["text"],
                    "apiKey": "private", "headers": {"Authorization": "private"}}
                   for provider in ["a", "b"]]
        models = discovery.normalize("omp", entries)
        self.assertEqual(len(models), 2)
        self.assertNotEqual(models[0]["key"], models[1]["key"])
        self.assertNotIn("private", json.dumps(models))
        self.assertIsNone(models[0]["reasoningEfforts"])
        other = discovery.model("codex", "a/same", "Same name")
        self.assertNotEqual(models[0]["key"], other["key"])

    def test_invalid_and_duplicate_catalogs_are_rejected(self):
        for entries in [None, {}, [None], [{"model": "a"}],
                        [{"model": "a", "displayName": "A", "supportedReasoningEfforts": ["high"]}]]:
            with self.subTest(entries=entries), self.assertRaises(discovery.DiscoveryError):
                discovery.normalize("codex", entries)
        item = {"model": "a", "displayName": "A"}
        with self.assertRaisesRegex(discovery.DiscoveryError, "duplicate"):
            discovery.normalize("codex", [item, item])
        with self.assertRaises(discovery.DiscoveryError):
            discovery.normalize("omp", [{"provider": "a", "id": "b", "selector": "c", "name": "B"}])
        self.assertEqual(discovery.normalize("codex", [dict(item, hidden=True)]), [])

    def test_pagination_cycle_and_private_errors_fail_closed(self):
        for payload, error in [
            ({"result": {"data": [], "nextCursor": "repeat"}}, "repeated"),
            ({"error": {"message": "credential-secret"}}, "rejected"),
        ]:
            with self.subTest(error=error):
                binary = self.executable(f'''
import json,sys
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 body={{'result':{{}}}} if v['method']=='initialize' else {payload!r}
 print(json.dumps({{'id':v['id'],**body}}),flush=True)
''')
                with self.assertRaisesRegex(discovery.DiscoveryError, error) as caught:
                    discovery.codex_models(binary, self.root, 2)
                self.assertNotIn("credential-secret", str(caught.exception))

    def test_timeout_and_output_limits_reap_the_probe(self):
        for source, expected in [("import time;time.sleep(30)", "timed out"),
                                 ("import sys;sys.stdout.write('x'*10000);sys.stdout.flush()", "output limit")]:
            with self.subTest(expected=expected):
                binary = self.executable(source)
                started = time.monotonic()
                with patch.object(discovery, "MAX_OUTPUT", 1024):
                    with self.assertRaisesRegex(discovery.DiscoveryError, expected):
                        with discovery.Process([binary], self.root, .1) as process:
                            process.collect()
                self.assertIsNotNone(process.proc.returncode)
                self.assertLess(time.monotonic() - started, 2)

    def test_missing_executable_is_reported_without_blocking_another_harness(self):
        missing = discovery.discover("codex", str(self.root / "absent"))
        self.assertEqual(missing["status"], "unavailable")
        binary = self.executable('''
import json,sys
if '--version' in sys.argv: print('fixture 1.2.3')
else:
 assert sys.argv[1:]==['models','--json','--no-extensions']
 print(json.dumps({'models':[]}))
''')
        other = discovery.discover("omp", binary, 2)
        self.assertEqual(other["status"], "ok")
        self.assertEqual(other["version"], "1.2.3")
        self.assertEqual(other["models"], [])

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux process-state receipt")
    def test_exited_leader_does_not_leave_a_helper_running(self):
        pid_file = self.root / "helper.pid"
        binary = self.executable(f'''
import subprocess,sys
p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],
                   stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
open({str(pid_file)!r},'w').write(str(p.pid))
print('{{}}',flush=True)
''')
        with discovery.Process([binary], self.root, 2) as process:
            self.assertEqual(json.loads(process.collect()), {})
        pid = int(pid_file.read_text())
        until = time.monotonic() + 2
        while time.monotonic() < until:
            try:
                status = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
            except FileNotFoundError:
                return
            if status == "Z":
                return  # Already dead; the OS's adopter owns reaping this orphan.
            time.sleep(.01)
        self.fail("Discovery helper survived its process group cleanup")

    def test_partial_or_malformed_json_is_bounded_and_not_echoed(self):
        for body in ["credential-secret\n", "{partial"]:
            binary = self.executable(f"import sys;sys.stdout.write({body!r});sys.stdout.flush()")
            with self.assertRaises(discovery.DiscoveryError) as caught:
                with discovery.Process([binary], self.root, 1) as process:
                    process.receive(lambda _: True)
            self.assertNotIn("credential-secret", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
