"""Loopback Mail fault: retain a committed send while its caller loses the reply."""
import http.server
import json
import threading
import time
import urllib.request

class CommittedSendProxy:
    def __init__(self, root, mail):
        self.root, self.committed, self.mail = root, [], mail
        self.external_once = False
        self.stop = threading.Event()
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def do_POST(self):
                raw = self.rfile.read(int(self.headers['Content-Length']))
                payload = json.loads(raw)
                request = urllib.request.Request(f'http://127.0.0.1:{mail.port}/mcp/', data=raw,
                    headers={key:self.headers[key] for key in ('Content-Type','Accept','Authorization','Mcp-Session-Id') if key in self.headers})
                with urllib.request.urlopen(request, timeout=12) as response:
                    data, status, headers = response.read(), response.status, dict(response.headers)
                args = payload.get('params', {}).get('arguments', {})
                if payload.get('params', {}).get('name') == 'send_message':
                    owner.committed.append({'arguments':args, 'response':data.decode()})
                    if args.get('body_md') == 'COMMITTED_BEFORE_CRASH' and len(owner.committed) == 1:
                        committed=json.loads(json.loads(data)['result']['content'][0]['text'])
                        assert committed['count']==1 and committed['deliveries'][0]['payload']['id']
                        (root/'send-committed').write_text(data.decode())
                        until = time.monotonic()+30
                        while not (root/'release-send').exists() and not owner.stop.wait(.02) and time.monotonic()<until: pass
                        self.close_connection = True
                        return
                self.send_response(status)
                for key in ('Content-Type','Mcp-Session-Id'):
                    if key in headers: self.send_header(key,headers[key])
                self.end_headers()
                try: self.wfile.write(data)
                except (BrokenPipeError,ConnectionResetError): pass
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.port = self.server.server_port
        threading.Thread(target=self.server.serve_forever,daemon=True).start()
    def close(self):
        self.stop.set(); self.server.shutdown(); self.server.server_close()
        sends=[row for row in self.committed if row['arguments'].get('body_md')=='COMMITTED_BEFORE_CRASH']
        if sends:
            args=sends[0]['arguments']
            inbox=self.mail.call('fetch_inbox',project_key=args['project_key'],agent_name=args['to'][0],include_bodies=True,mark_read=False,limit=100)
            (self.root/'external-inbox.json').write_text(json.dumps(inbox,indent=2)+'\n')
            self.external_once=sum(row.get('subject')==args['idempotency_key'] for row in inbox)==1
        (self.root/'send-proxy.json').write_text(json.dumps(self.committed,indent=2)+'\n')
