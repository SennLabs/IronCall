"""Phase 4 E2E: cookies across a redirect, inherited folder auth and collection default
headers, and a multipart upload of a workspace file, against the real app.
(Proxy, custom CA and verify-off are covered by the engine's integration tests.)"""
import json, os, sys, tempfile, threading, time
from email import policy
from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, Driver, data_env, restore_workspace  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
seen = []


class H(BaseHTTPRequestHandler):
    def reply(self, status, body, headers=()):
        data = json.dumps(body).encode()
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, extra=None):
        seen.append({"method": self.command, "path": self.path, "cookie": self.headers.get("Cookie"),
                     "auth": self.headers.get("Authorization"), "client": self.headers.get("X-Client"),
                     **(extra or {})})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if self.path == "/login":
            body = self.rfile.read(length).decode()
            self.record({"body": body})
            self.send_response(302)
            self.send_header("Set-Cookie", "session=abc123; Path=/; HttpOnly")
            self.send_header("Location", "/me")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif self.path == "/upload":
            raw = self.rfile.read(length)
            msg = BytesParser(policy=policy.default).parsebytes(
                b"Content-Type: " + self.headers["Content-Type"].encode() + b"\r\n\r\n" + raw)
            parts, filename = {}, None
            for part in msg.iter_parts():
                name = part.get_param("name", header="content-disposition")
                payload = part.get_payload(decode=True)
                if part.get_filename():
                    filename = part.get_filename()
                    parts[name] = len(payload)
                else:
                    parts[name] = payload.decode()
            self.record({"parts": parts, "filename": filename})
            self.reply(200, {"parts": parts})

    def do_GET(self):
        self.record()
        if self.path == "/me":
            ok = "session=abc123" in (self.headers.get("Cookie") or "")
            self.reply(200 if ok else 401, {"me": ok})

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
base = f"http://127.0.0.1:{srv.server_address[1]}"
threading.Thread(target=srv.serve_forever, daemon=True).start()

tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "ws")
os.makedirs(os.path.join(data, APP_ID))
os.makedirs(os.path.join(ws, "api", "files"))
payload = os.urandom(300_000)
open(os.path.join(ws, "api", "files", "payload.bin"), "wb").write(payload)

open(os.path.join(ws, "ironcall.yaml"), "w").write(
    "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8J9'\nname: 'Net E2E'\n"
    f"vars:\n  base: '{base}'\n  token: 'folder-token-1'\n"
    "headers:\n- {name: 'X-Client', value: 'ironcall-e2e', enabled: true}\n")
open(os.path.join(ws, "login.yaml"), "w").write(
    "schema: 'ironcall/request@1'\nid: '01J9Z3K7T2V8Q4M6N0P5R1S3A1'\nname: 'Login'\nmethod: 'POST'\n"
    "url: '{{base}}/login'\nbody:\n  type: 'formUrlencoded'\n  fields:\n  - {name: 'user', value: 'ada', enabled: true}\n")
open(os.path.join(ws, "api", "_folder.yaml"), "w").write(
    "schema: 'ironcall/folder@1'\nid: '01J9Z3K7T2V8Q4M6N0P5R1S3F1'\nname: 'API'\n"
    "auth:\n  type: 'bearer'\n  token: '{{token}}'\n")
open(os.path.join(ws, "api", "me.yaml"), "w").write(
    "schema: 'ironcall/request@1'\nid: '01J9Z3K7T2V8Q4M6N0P5R1S3A2'\nname: 'Me'\nmethod: 'GET'\nurl: '{{base}}/me'\n")
open(os.path.join(ws, "api", "upload.yaml"), "w").write(
    "schema: 'ironcall/request@1'\nid: '01J9Z3K7T2V8Q4M6N0P5R1S3A3'\nname: 'Upload'\nmethod: 'POST'\n"
    "url: '{{base}}/upload'\nbody:\n  type: 'multipart'\n  parts:\n"
    "  - {name: 'note', type: 'text', value: 'hello', enabled: true}\n"
    "  - {name: 'file', type: 'file', path: 'api/files/payload.bin', enabled: true}\n")
restore_workspace(data, ws)

d = Driver(OUT, env=data_env(data))
try:
    def open_request(name):
        d.click_on(css=f'[role="treeitem"][aria-label="{name}"]')
        time.sleep(0.6)

    def send(expect):
        before = len(seen)
        d.click_on(xpath='//button[normalize-space()="Send"]')
        return d.wait(lambda: len(seen) >= before + expect, timeout=15)

    open_request("Login")
    d.check(send(2), "login sent and redirect followed")
    login, me = seen[-2], seen[-1]
    d.check(login["body"] == "user=ada", f"form body sent ({login['body']!r})")
    d.check(login["auth"] is None, "root request has no auth (collection has none)")
    d.check(login["client"] == "ironcall-e2e", "collection default header sent")
    d.check(me["path"] == "/me" and "session=abc123" in (me["cookie"] or ""), "cookie set by 302 sent on the redirect hop")

    d.click_on(css='[role="treeitem"][aria-label="API"]')  # expand the folder
    open_request("Me")
    d.check(send(1), "folder request sent")
    me = seen[-1]
    d.check(me["auth"] == "Bearer folder-token-1", f"folder Bearer auth inherited, variable resolved ({me['auth']})")
    d.check("session=abc123" in (me["cookie"] or ""), "cookie jar reused for a later request")
    d.shot("net-1-me")

    open_request("Upload")
    d.check(send(1), "upload sent")
    up = seen[-1]
    d.check(up.get("parts") == {"note": "hello", "file": len(payload)}, f"multipart text + {len(payload)}-byte file received ({up.get('parts')})")
    d.check(up.get("filename") == "payload.bin", "file part has its file name")
    d.shot("net-2-upload")

    # Cookies dialog lists the session cookie.
    d.click_on(css='[aria-label="Workspace"]')
    d.click_on(xpath='//button[normalize-space()="Cookies…"]')
    table = d.find(css='table[aria-label="Cookies"]')
    d.check(d.wait(lambda: "session" in d.text(table)), "cookies dialog shows the session cookie")
    d.shot("net-3-cookies")
finally:
    d.quit()

jar = [os.path.join(r, f) for r, _, fs in os.walk(os.path.join(data, APP_ID, "cookies")) for f in fs]
d.check(len(jar) == 1 and "abc123" in open(jar[0]).read(), "cookie jar persisted in the app data dir (not the workspace)")
d.check(not any("abc123" in open(os.path.join(r, f), errors="ignore").read()
                for r, _, fs in os.walk(ws) for f in fs if not f.endswith(".bin")), "no cookie value in workspace files")
sys.exit(0 if d.ok else 1)
