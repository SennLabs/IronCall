"""Phase 7 E2E: assertions, extraction and the collection runner against the real app.

Exit criterion (PLAN.md Phase 7): a login → create → fetch → delete flow against a test
server runs with chained IDs. Also checks a single send's Tests tab, and that the
extracted (sensitive) token never reaches workspace files, logs or session state.
"""
import json, os, sys, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, Driver, data_env, restore_workspace  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
TOKEN = "tok-runner-e2e-5f4e3d"
users, seen = [], []


class H(BaseHTTPRequestHandler):
    def reply(self, status, body=None):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authed(self):
        return self.headers.get("Authorization") == f"Bearer {TOKEN}"

    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        seen.append(f"{self.command} {self.path}")
        if self.command == "POST" and self.path == "/login":
            return self.reply(200, {"access_token": TOKEN})
        if not self.authed():
            return self.reply(401, {"error": "unauthorized"})
        if self.command == "POST" and self.path == "/users":
            users.append(f"u{len(users) + 1}")
            return self.reply(201, {"id": users[-1]})
        uid = self.path.rsplit("/", 1)[-1]
        if self.path.startswith("/users/") and uid in users:
            if self.command == "GET":
                return self.reply(200, {"id": uid, "name": "Ada"})
            if self.command == "DELETE":
                users.remove(uid)
                return self.reply(204)
        return self.reply(404, {"error": "not found"})

    do_GET = do_POST = do_DELETE = handle_any

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{port}"

tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "ws")
os.makedirs(os.path.join(data, APP_ID))
os.makedirs(ws)

IDS = {
    "login": "01J9Z3K7T2V8Q4M6N0P5R1S3A1",
    "create": "01J9Z3K7T2V8Q4M6N0P5R1S3A2",
    "fetch": "01J9Z3K7T2V8Q4M6N0P5R1S3A3",
    "delete": "01J9Z3K7T2V8Q4M6N0P5R1S3A4",
}
BEARER = "headers:\n- {name: 'Authorization', value: 'Bearer {{authToken}}', enabled: true}\n"
REQUESTS = {
    "login": ("Login", "POST", "/login", "",
              "extract:\n- {var: 'authToken', from: 'body', path: '$.access_token'}\n"
              "assert:\n- {target: 'status', op: 'eq', value: 200}\n"),
    "create": ("Create user", "POST", "/users", BEARER,
               "extract:\n- {var: 'userId', from: 'body', path: '$.id'}\n"
               "assert:\n- {target: 'status', op: 'eq', value: 201}\n"),
    "fetch": ("Fetch user", "GET", "/users/{{userId}}", BEARER,
              "assert:\n- {target: 'status', op: 'eq', value: 200}\n"
              "- {target: 'body', path: '$.id', op: 'eq', value: '{{userId}}'}\n"
              "- {target: 'duration', op: 'lt', value: 5000}\n"),
    "delete": ("Delete user", "DELETE", "/users/{{userId}}", BEARER,
               "assert:\n- {target: 'status', op: 'in', value: [200, 204]}\n"),
}
open(os.path.join(ws, "ironcall.yaml"), "w").write(
    "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8R1'\nname: 'Runner E2E'\n"
    "order:\n" + "".join(f"- '{IDS[k]}'\n" for k in IDS))
for key, (name, method, path, headers, checks) in REQUESTS.items():
    open(os.path.join(ws, f"{key}.yaml"), "w").write(
        f"schema: 'ironcall/request@1'\nid: '{IDS[key]}'\nname: '{name}'\nmethod: '{method}'\n"
        f"url: '{BASE}{path}'\n{headers}{checks}")
files_before = {f: open(os.path.join(ws, f)).read() for f in os.listdir(ws) if f.endswith(".yaml")}
restore_workspace(data, ws)

d = Driver(OUT, env=data_env(data))
try:
    # A single send runs the request's assertions and extraction.
    d.click_on(css='[role="treeitem"][aria-label="Login"]')
    d.click_on(xpath='//button[normalize-space()="Send"]')
    tests_tab = d.find(xpath='//*[@role="tablist" and @aria-label="Response"]/*[@role="tab" and starts-with(normalize-space(),"Tests")]', timeout=20)
    d.check(d.text(tests_tab).startswith("Tests 1/1"), f"response Tests tab shows 1/1 ({d.text(tests_tab)!r})")
    d.click(tests_tab)
    status = d.find(xpath='//*[@role="status" and contains(normalize-space(),"assertions passed")]')
    d.check("1 of 1 assertions passed" in d.text(status), "assertion passed after a single send")
    extracted = d.find(css='[aria-label="Extracted authToken"]')
    d.check("••••" in d.text(extracted) and TOKEN not in d.text(extracted), "extracted token is masked")
    d.shot("runner-1-tests-tab")

    # The runner: login → create → fetch → delete with chained values.
    d.click_on(xpath='//header//button[normalize-space()="Run"]')
    dialog = '//*[@role="dialog" and starts-with(@aria-label,"Run: ")]'
    d.find(xpath=f'{dialog}//*[normalize-space()="4 requests, in order."]')
    t0 = time.time()
    d.click_on(xpath=f'{dialog}//button[normalize-space()="Run"]')
    summary = d.find(css='[aria-label="Run summary"]', timeout=30)
    text = d.text(summary)
    d.check(text.startswith("4 passed, 0 failed, 0 errors"), f"run summary: {text!r}")
    rows = [d.text(r) for r in d.find_all('[aria-label="Run results"] tbody tr')]
    d.check(len(rows) == 4 and all("Passed" in r for r in rows), f"every request passed ({rows})")
    d.check(seen[-4:] == ["POST /login", "POST /users", "GET /users/u1", "DELETE /users/u1"],
            f"server saw the chained flow in order ({seen[-4:]})")
    d.check(users == [], "the created user was deleted using the extracted ID")
    print(f"INFO run took {time.time() - t0:.2f}s")

    d.click_on(css='[aria-label="Details of Fetch user"]')
    detail = d.text(d.find(xpath=f'{dialog}//ul[@aria-label="Assertion results"]'))
    d.check('body $.id eq "{{userId}}"' in detail, "assertion labels show the authored {{reference}}")
    d.shot("runner-2-results")

    # Drill-down: the result opens its history entry.
    d.click_on(xpath=f'{dialog}//button[normalize-space()="Open in history"]')
    d.check(d.wait(lambda: not d.exists(xpath=dialog, timeout=0.5)), "runner closed for the drill-down")
    d.check(d.exists(css='[aria-label="History"]'), "history panel open")
    d.shot("runner-3-history")

    # The built-in echo server answers {{$localEcho}} (the sample workspace relies on it).
    d.click_on(xpath='//header//button[normalize-space()="History"]')
    d.wait(lambda: not d.exists(css='[aria-label="History"]', timeout=0.5))
    d.click_on(css='[aria-label="New scratch request"]')
    # The view remounts for the new tab: look the URL box up again once it's empty.
    d.wait(lambda: d.prop(d.find(css='input[aria-label="URL"]'), "value") == "", timeout=5)
    d.type_into("{{$localEcho}}/anything/e2e?x=1", css='input[aria-label="URL"]')
    d.click_on(xpath='//select[@aria-label="Method"]/option[normalize-space()="POST"]')
    d.click_on(xpath='//button[normalize-space()="Send"]')
    status = d.find(xpath='//*[@aria-label="Response"]//*[normalize-space()="200 OK"]', timeout=20)
    d.check(status is not None, "{{$localEcho}} reaches the built-in echo server")
    body = d.text(d.find(css='[aria-label="Response body"]'))
    d.check('"/anything/e2e"' in body, "echo shows the request path")
    d.check('"method": "POST"' in body, "the method dropdown sets the method (POST)")
    d.shot("runner-4-method")
finally:
    d.quit()

files_after = {f: open(os.path.join(ws, f)).read() for f in files_before}
d.check(files_after == files_before, "running never rewrites request files")

# The token must never reach workspace files, logs or session state. History stores
# response bodies (the login body holds the token by design) and the spool holds the
# raw responses while the app runs; those are checked by Rust tests for their metadata.
leaks = []
for base in (ws, data):
    for root, _, names in os.walk(base):
        for name in names:
            path = os.path.join(root, name)
            if ".sqlite" in name or f"{os.sep}spool" in path or f"{os.sep}webview" in path:
                continue
            try:
                if TOKEN.encode() in open(path, "rb").read():
                    leaks.append(os.path.relpath(path, tmp))
            except OSError:
                pass
d.check(not leaks, f"token not found in workspace files, logs or app data (leaks: {leaks})")
sys.exit(0 if d.ok else 1)
