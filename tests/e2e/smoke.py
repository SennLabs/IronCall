"""End-to-end smoke test: drives the real app over WebDriver.

Uses tauri-driver + WebKitWebDriver (Linux) or msedgedriver (Windows). Plain WebDriver
over HTTP, so it needs only the Python standard library. Run with `just e2e`.

Usage: smoke.py [SCREENSHOT_DIR]   (env IRONCALL_APP overrides the app binary)
"""
import base64, json, sys, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import os
sys.path.insert(0, os.path.dirname(__file__))
from wd import APP, data_env, start_driver  # noqa: E402
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
os.makedirs(OUT, exist_ok=True)

ROW = b'  {"id": 12345, "name": "row-marker", "tags": ["a", "b"]},\n'
BIG = b"[\n" + ROW * (20 * 1024 * 1024 // len(ROW) + 1) + b'  {}\n]\n'

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/big"):
            self.send_response(200); self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(BIG))); self.end_headers(); self.wfile.write(BIG); return
        body = json.dumps({"hello": "world", "path": self.path, "items": [1, 2, 3]}).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass

srv = HTTPServer(("127.0.0.1", 0), H); port = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()

def wd(method, path, body=None):
    req = urllib.request.Request(f"http://127.0.0.1:4444{path}", method=method,
        data=None if body is None else json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or b"{}").get("value")

log = open(f"{OUT}/driver.log", "w")
import tempfile
# A fresh app data dir: no restored session, and the user's app data is never touched.
driver = start_driver(log, data_env(tempfile.mkdtemp(prefix="ironcall-e2e-")))
ok = True
try:
    sid = wd("POST", "/session", {"capabilities": {"alwaysMatch": {"tauri:options": {"application": APP}}}})["sessionId"]
    s = f"/session/{sid}"
    def find(css=None, xpath=None, timeout=15):
        end = time.time() + timeout
        while time.time() < end:
            try:
                v = wd("POST", f"{s}/element", {"using": "css selector", "value": css} if css else {"using": "xpath", "value": xpath})
                return list(v.values())[0]
            except urllib.error.HTTPError:
                time.sleep(0.2)
        raise RuntimeError(f"not found: {css or xpath}")
    def text(el): return wd("GET", f"{s}/element/{el}/text")
    def shot(name):
        open(f"{OUT}/{name}.png", "wb").write(base64.b64decode(wd("GET", f"{s}/screenshot")))
    def check(cond, msg):
        global ok
        print(("PASS " if cond else "FAIL ") + msg); ok &= bool(cond)

    def find_all(css):
        return [list(v.values())[0] for v in wd("POST", f"{s}/elements", {"using": "css selector", "value": css})]
    def type_into(el, txt):
        wd("POST", f"{s}/element/{el}/value", {"text": txt})
    def click(el): wd("POST", f"{s}/element/{el}/click", {})
    def send_btn(): return find(xpath='//button[normalize-space()="Send"]')
    def wait_text(css, needle, timeout=15):
        end = time.time() + timeout
        while time.time() < end:
            try:
                t = text(find(css=css, timeout=1))
                if needle in t: return t
            except Exception: pass
            time.sleep(0.2)
        return None

    url = find(css='input[aria-label="URL"]')
    wd("POST", f"{s}/element/{url}/clear", {})
    type_into(url, f"http://127.0.0.1:{port}/users/:id?page=2")
    time.sleep(0.4)
    # Fill the path param (last "Value" input on the Params tab).
    type_into(find_all('input[placeholder="Value"]')[-1], "42")
    time.sleep(0.4)
    click(send_btn())
    st = wait_text('[data-testid="status"]', "200")
    shot("2-response")
    check(st is not None, f"status shows 200 (got {st!r})")
    panel = text(find(css='section[aria-label="Response"]'))
    check('"/users/42?page=2"' in panel or "/users/42?page=2" in panel, "server saw /users/42?page=2 (path param + query encoded)")
    check("world" in panel, "JSON body rendered")

    # Error case: nothing listens on port 9.
    wd("POST", f"{s}/element/{url}/clear", {})
    type_into(url, "http://127.0.0.1:9/")
    time.sleep(0.4)
    click(send_btn())
    time.sleep(1.5)
    panel = text(find(css='section[aria-label="Response"]'))
    shot("3-error")
    check("refused" in panel.lower() or "connect" in panel.lower(), "connection error explained: " + panel[:120].replace("\n", " | "))

    # 20MB response: exit criterion is first page visible in under 1s.
    wd("POST", f"{s}/element/{url}/clear", {})
    type_into(url, f"http://127.0.0.1:{port}/big")
    time.sleep(0.4)
    t0 = time.time()
    click(send_btn())
    seen = wait_text('[aria-label="Response body"]', "row-marker", timeout=20)
    elapsed = time.time() - t0
    shot("4-big")
    # Includes WebDriver polling overhead (~0.2s granularity), so it overstates the real time.
    check(seen is not None and elapsed < 1.5, f"20MB body: first rows visible after {elapsed:.2f}s (incl. polling)")

    # Dark theme: cycle System -> Light -> Dark.
    theme = find(xpath='//button[starts-with(normalize-space(),"Theme:")]')
    for _ in range(3):  # the preference persists between runs, so cycle until Dark
        if text(theme).strip() == "Theme: Dark":
            break
        click(theme)
    check(text(theme).strip() == "Theme: Dark", "dark theme selected")
    wd("POST", f"{s}/element/{url}/clear", {})
    type_into(url, f"http://127.0.0.1:{port}/users/:id?page=2")
    time.sleep(0.4)
    type_into(find_all('input[placeholder="Value"]')[-1], "7")
    time.sleep(0.4)
    click(send_btn())
    check(wait_text('[data-testid="status"]', "200") is not None, "dark theme: send works")
    time.sleep(0.5)
    shot("5-dark")
finally:
    try: wd("DELETE", f"/session/{sid}")
    except Exception: pass
    driver.terminate()
sys.exit(0 if ok else 1)
