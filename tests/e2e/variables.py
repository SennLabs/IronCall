"""Phase 3 E2E: environments, variables and secrets against the real app.

A fresh clone of a workspace declares a secret but has no value for it, so the
missing-secrets banner must appear; the value is entered through the UI, sent in a
header, and must never reach workspace files or logs. Switching environments
changes the resolved URL. Uses the plaintext secret backend because the container
has no OS keychain; a temporary app data dir keeps the user's data untouched.
"""
import json, os, sys, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, Driver, data_env, restore_workspace  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
SECRET = "tok-9f8e7d6c5b4a"
seen = []


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        seen.append({"path": self.path, "token": self.headers.get("X-Token")})
        body = json.dumps({"path": self.path}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()

tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "ws")
os.makedirs(os.path.join(data, APP_ID))
os.makedirs(os.path.join(ws, "environments"))
os.makedirs(os.path.join(ws, ".ironcall"))

DEV, PROD = "01J9Z3M0A1B2C3D4E5F6G7H8D1", "01J9Z3M0A1B2C3D4E5F6G7H8P2"
open(os.path.join(ws, "ironcall.yaml"), "w").write(
    "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8J9'\nname: 'Vars E2E'\n")
for env_id, name, path in [(DEV, "Dev", "dev"), (PROD, "Prod", "prod")]:
    open(os.path.join(ws, "environments", f"{path}.yaml"), "w").write(
        f"schema: 'ironcall/environment@1'\nid: '{env_id}'\nname: '{name}'\nvars:\n"
        f"  baseUrl: 'http://127.0.0.1:{port}/{path}'\n  token:\n    secret: true\n")
open(os.path.join(ws, "get-user.yaml"), "w").write(
    "schema: 'ironcall/request@1'\nid: '01J9Z3K7T2V8Q4M6N0P5R1S3W7'\nname: 'Get user'\nmethod: 'GET'\n"
    "url: '{{baseUrl}}/users'\nheaders:\n- {name: 'X-Token', value: '{{token}}', enabled: true}\n")
# As if the user had chosen Dev (local, untracked settings) and the plaintext backend
# (kept per machine in the app data folder).
open(os.path.join(ws, ".ironcall", "local.yaml"), "w").write(f"activeEnvironment: '{DEV}'\n")
restore_workspace(data, ws, secret_backend="plaintextFile")

d = Driver(OUT, env=data_env(data))
try:
    banner = d.find(css='[aria-label="Missing secrets"]')
    d.check("missing" in d.text(banner).lower() and "Dev" in d.text(banner), "fresh clone: missing-secrets banner for Dev")
    d.shot("vars-1-banner")

    d.click_on(xpath='//*[@aria-label="Missing secrets"]//button[normalize-space()="Enter now"]')
    d.type_into(SECRET, css='input[aria-label^="token ("]')
    d.click_on(css='button[type="submit"][form="fill-secrets"]')
    gone = d.wait(lambda: not d.exists(css='[aria-label="Missing secrets"]', timeout=0.5))
    d.check(gone, "banner cleared after entering the secret")

    d.click_on(css='[role="treeitem"][aria-label="Get user"]')
    send = lambda: d.click_on(xpath='//button[normalize-space()="Send"]')
    send()
    d.check(d.wait(lambda: len(seen) >= 1), "request sent")
    d.check(seen and seen[-1] == {"path": "/dev/users", "token": SECRET}, f"Dev: URL and secret header resolved ({seen[-1] if seen else None})")

    d.click_on(xpath='//select[@aria-label="Environment"]/option[starts-with(normalize-space(),"Prod")]')
    # Prod's token is a separate secret with no value yet: the banner is back for Prod.
    # (Sending now would ask "send anyway?" in a native dialog, which WebDriver can't
    # drive, so enter the value first.)
    banner = d.find(css='[aria-label="Missing secrets"]')
    d.check("Prod" in d.text(banner), "Prod's own secret reported missing")
    local = open(os.path.join(ws, ".ironcall", "local.yaml")).read()
    d.check(PROD in local, "active environment persisted in .ironcall/local.yaml")
    d.click_on(xpath='//*[@aria-label="Missing secrets"]//button[normalize-space()="Enter now"]')
    d.type_into(SECRET + "-prod", css='input[aria-label^="token ("]')
    d.click_on(css='button[type="submit"][form="fill-secrets"]')
    d.wait(lambda: not d.exists(css='[aria-label="Missing secrets"]', timeout=0.5))

    send()
    d.check(d.wait(lambda: len(seen) >= 2), "second request sent")
    d.check(seen[-1] == {"path": "/prod/users", "token": SECRET + "-prod"},
            f"Prod: switching environment changed the URL and secret ({seen[-1]})")
    d.shot("vars-2-prod")
finally:
    d.quit()

# The secret must never be in tracked files, the app's logs, or session state.
leaks = []
for base, skip in [(ws, os.path.join(ws, ".ironcall", "secrets.json")), (data, None)]:
    for root, _, files in os.walk(base):
        for name in files:
            path = os.path.join(root, name)
            if path == skip:
                continue
            try:
                if SECRET.encode() in open(path, "rb").read():
                    leaks.append(os.path.relpath(path, tmp))
            except OSError:
                pass
d.check(not leaks, f"secret not found in workspace files, logs or app data (leaks: {leaks})")
sys.exit(0 if d.ok else 1)
