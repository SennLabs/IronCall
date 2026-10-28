"""Phase 6 E2E: cURL import and export, and OpenAPI import, against the real app."""
import json, os, sys, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, Driver, data_env, restore_workspace  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
seen = []


class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
        seen.append({"method": "POST", "path": self.path, "demo": self.headers.get("X-Demo"),
                     "auth": self.headers.get("Authorization"), "body": body})
        data = b'{"ok": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()

tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "ws")
os.makedirs(os.path.join(data, APP_ID))
os.makedirs(ws)
open(os.path.join(ws, "ironcall.yaml"), "w").write(
    "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8J9'\nname: 'Import E2E'\n")
restore_workspace(data, ws)
collection_before = open(os.path.join(ws, "ironcall.yaml")).read()

CURL = (f"curl 'http://127.0.0.1:{port}/echo?x=1' \\\n  -H 'X-Demo: yes' \\\n"
        "  -H 'Authorization: Bearer topsecret-abc' \\\n  -H 'Content-Type: application/json' \\\n"
        "  --data-raw '{\"a\":1}' --compressed -sS")

d = Driver(OUT, env=data_env(data))


def open_import():
    d.click_on(css='[aria-label="Workspace"]')
    d.click_on(xpath='//button[normalize-space()="Import cURL / OpenAPI…"]')


def set_value(css, text):
    """Sets a textarea's value in one go and fires `input` (typing 5KB key by key is slow)."""
    d.execute("""const el = document.querySelector(arguments[0]);
        const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        setter.call(el, arguments[1]); el.dispatchEvent(new Event('input', {bubbles: true}));""", [css, text])


try:
    d.find(css='[role="treeitem"]')
    # --- cURL import
    open_import()
    set_value('textarea[aria-label="cURL command"]', CURL)
    preview = d.wait(lambda: "POST" in d.text(d.find(css='[aria-label="cURL preview"]', timeout=1)), timeout=10)
    d.check(preview, "cURL preview shows the parsed request")
    d.shot("imp-1-curl")
    d.click_on(xpath='//button[normalize-space()="Open in new tab"]')
    time.sleep(0.8)
    d.click_on(xpath='//button[normalize-space()="Send"]')
    d.check(d.wait(lambda: seen, timeout=10), "imported request sent")
    got = seen[-1] if seen else {}
    d.check(got.get("path") == "/echo?x=1" and got.get("demo") == "yes" and got.get("body") == '{"a":1}',
            f"method, URL+query, header and body imported ({got})")

    # --- Copy as cURL (default: resolved, secrets masked)
    d.click_on(xpath='//*[@role="tab" and normalize-space()="Request"]')
    d.click_on(xpath='//button[normalize-space()="Copy as cURL…"]')
    command = d.wait(lambda: d.prop(d.find(css='textarea[aria-label="cURL command"]', timeout=1), "value"), timeout=10) or ""
    d.shot("imp-2-copy")
    d.check(f"127.0.0.1:{port}/echo" in command and "X-Demo" in command, "Copy as cURL builds the command")
    d.check("topsecret-abc" not in command, "Authorization value masked by default")
    d.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))")
    time.sleep(0.5)

    # --- OpenAPI import (pasted text)
    open_import()
    d.click_on(xpath='//*[@role="tab" and normalize-space()="OpenAPI / Swagger"]')
    d.click_on(xpath='//summary[normalize-space()="Or paste the spec"]')
    spec = open(os.path.join(ROOT, "tests/fixtures/openapi/petstore-v3.0.yaml")).read()
    set_value('textarea[aria-label="Spec text"]', spec)
    d.click_on(xpath='//details//button[normalize-space()="Preview"]')
    planned = d.wait(lambda: "List all pets" in d.text(d.find(css='[aria-label="Import preview"]', timeout=1)), timeout=10)
    d.check(planned, "OpenAPI preview lists the operations")
    d.shot("imp-3-openapi-preview")
    d.click_on(xpath='//*[@role="dialog"]//button[normalize-space()="Import"]')
    folder = d.wait(lambda: d.exists(css='[role="treeitem"][aria-label="Swagger Petstore"]', timeout=1), timeout=15)
    d.check(folder, "imported folder appears in the tree")
    active = d.wait(lambda: "Swagger Petstore" in (d.execute(
        "const s = document.querySelector('select[aria-label=\"Environment\"]'); return s ? s.options[s.selectedIndex].text : '';") or ""), timeout=5)
    d.check(active, "new environment is active in the switcher")
    d.shot("imp-4-imported")
finally:
    d.quit()

yaml_files = [os.path.join(r, f) for r, _, fs in os.walk(ws) for f in fs if f.endswith(".yaml") and ".ironcall" not in r]
requests = [p for p in yaml_files if "ironcall/request@1" in open(p).read()]
d.check(len(requests) >= 3, f"OpenAPI requests written to disk ({len(requests)})")
envs = os.listdir(os.path.join(ws, "environments")) if os.path.isdir(os.path.join(ws, "environments")) else []
d.check(envs and "petstore.swagger.io" in open(os.path.join(ws, "environments", envs[0])).read(),
        f"environment created with baseUrl from servers ({envs})")
d.check(open(os.path.join(ws, "ironcall.yaml")).read() == collection_before, "collection file left untouched by the import")
sys.exit(0 if d.ok else 1)
