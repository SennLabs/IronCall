"""Minimal W3C WebDriver client for the E2E tests (standard library only)."""
import base64, json, os, subprocess, sys, time, urllib.error, urllib.request

WINDOWS = sys.platform == "win32"
APP = os.path.abspath(os.environ.get("IRONCALL_APP") or os.path.join(
    os.path.dirname(__file__), "../../target/debug/ironcall" + (".exe" if WINDOWS else "")))
# The app's data folder name under XDG_DATA_HOME (the bundle identifier).
APP_ID = "io.ironcall.desktop"
# Performance budgets (PLAN.md) only mean something for release builds on real hardware
# with GPU rendering. Under Xvfb software rendering they are reported, not enforced,
# unless IRONCALL_E2E_PERF=1.
ENFORCE_PERF = f"{os.sep}release{os.sep}" in APP and os.environ.get("IRONCALL_E2E_PERF") == "1"


def data_env(data_home):
    """Env for an app whose data lives in `data_home/APP_ID`, never in the user's own.

    XDG_DATA_HOME covers Linux; IRONCALL_DATA_DIR also covers Windows, which ignores XDG."""
    return {"XDG_DATA_HOME": data_home, "IRONCALL_DATA_DIR": os.path.join(data_home, APP_ID)}


def restore_workspace(data_home, ws, secret_backend=None):
    """Makes the app reopen `ws` on start, as if it was open when the app last closed.

    The app only opens folders picked in its native dialog or on its recent list, so the
    workspace goes on the recent list too. `secret_backend` (e.g. "plaintextFile") is the
    backend chosen for it on this machine."""
    folder = os.path.join(data_home, APP_ID)
    if secret_backend:
        with open(os.path.join(folder, "secret-backends.json"), "w") as f:
            json.dump({os.path.realpath(ws): secret_backend}, f)
    with open(os.path.join(folder, "session.json"), "w") as f:
        json.dump({"workspace": ws, "tabs": [], "active": None}, f)
    with open(os.path.join(folder, "recent-workspaces.json"), "w") as f:
        json.dump([{"path": ws, "name": "E2E", "lastOpened": "2026-01-01T00:00:00Z"}], f)


def start_driver(log, env=None):
    """Starts tauri-driver on port 4444. On Windows, `run.py` finds a matching msedgedriver."""
    cmd = ["tauri-driver", "--port", "4444"]
    native = os.environ.get("IRONCALL_E2E_NATIVE_DRIVER")
    if native:
        cmd += ["--native-driver", native]
    proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, env={**os.environ, **(env or {})})
    time.sleep(1.5)
    return proc


CTRL = ""
RELEASE = ""


def call(method, path, body=None):
    req = urllib.request.Request(f"http://127.0.0.1:4444{path}", method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read() or b"{}").get("value")
    except urllib.error.HTTPError as e:
        # Keep WebDriver's explanation (e.g. "element click intercepted") in the error.
        try:
            detail = json.loads(e.read() or b"{}").get("value", {})
            e.msg = f"{e.msg}: {detail.get('error')}: {detail.get('message', '')[:300]}"
        except ValueError:
            pass
        raise


class Driver:
    """Starts tauri-driver (with `env` for the app) and one app session."""

    def __init__(self, out_dir, env=None):
        self.out = out_dir
        os.makedirs(out_dir, exist_ok=True)
        log = open(os.path.join(out_dir, "driver.log"), "a")
        self.proc = start_driver(log, env)
        self.ok = True
        try:
            self.start()
        except BaseException:
            # The caller never gets `self` to quit, so don't leave the driver holding the port.
            self.proc.terminate()
            raise

    def start(self):
        caps = {"capabilities": {"alwaysMatch": {"tauri:options": {"application": APP}}}}
        self.s = f"/session/{call('POST', '/session', caps)['sessionId']}"

    def stop_session(self):
        try:
            call("DELETE", self.s)
        except Exception:
            pass

    def quit(self):
        self.stop_session()
        self.proc.terminate()

    def find(self, css=None, xpath=None, timeout=15):
        end = time.time() + timeout
        q = {"using": "css selector", "value": css} if css else {"using": "xpath", "value": xpath}
        while True:
            try:
                return list(call("POST", f"{self.s}/element", q).values())[0]
            except urllib.error.HTTPError:
                if time.time() > end:
                    raise RuntimeError(f"not found: {css or xpath}")
                time.sleep(0.2)

    def _fresh(self, action, css, xpath, timeout=15, attempts=5):
        """Finds an element and runs `action` on it, finding it again if it was re-rendered
        in between ("stale element reference")."""
        for i in range(attempts):
            try:
                return action(self.find(css, xpath, timeout))
            except urllib.error.HTTPError as e:
                if "stale element" not in str(e.msg) or i == attempts - 1:
                    raise
                time.sleep(0.2)

    def click_on(self, css=None, xpath=None, timeout=15):
        return self._fresh(self.click, css, xpath, timeout)

    def type_into(self, txt, css=None, xpath=None, timeout=15):
        return self._fresh(lambda el: self.type(el, txt), css, xpath, timeout)

    def find_all(self, css):
        return [list(v.values())[0] for v in call("POST", f"{self.s}/elements", {"using": "css selector", "value": css})]

    def exists(self, css=None, xpath=None, timeout=3):
        try:
            self.find(css, xpath, timeout)
            return True
        except RuntimeError:
            return False

    def text(self, el):
        return call("GET", f"{self.s}/element/{el}/text")

    def prop(self, el, name):
        return call("GET", f"{self.s}/element/{el}/property/{name}")

    def type(self, el, txt):
        call("POST", f"{self.s}/element/{el}/value", {"text": txt})

    def clear(self, el):
        call("POST", f"{self.s}/element/{el}/clear", {})

    def click(self, el):
        call("POST", f"{self.s}/element/{el}/click", {})

    def execute_async(self, script, args=()):
        return call("POST", f"{self.s}/execute/async", {"script": script, "args": list(args)})

    def execute(self, script, args=()):
        return call("POST", f"{self.s}/execute/sync", {"script": script, "args": list(args)})

    def shot(self, name):
        with open(os.path.join(self.out, f"{name}.png"), "wb") as f:
            f.write(base64.b64decode(call("GET", f"{self.s}/screenshot")))

    def wait(self, cond, timeout=10):
        end = time.time() + timeout
        while time.time() < end:
            try:
                v = cond()
                if v:
                    return v
            except Exception:
                pass
            time.sleep(0.2)
        return None

    def check(self, cond, msg):
        print(("PASS " if cond else "FAIL ") + msg)
        self.ok &= bool(cond)
