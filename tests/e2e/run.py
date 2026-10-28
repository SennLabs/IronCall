"""Runs the E2E suites against a built app, on Linux or Windows. Used by `just e2e`.

Usage: run.py [SUITE ...]   (default: all suites, e.g. `run.py smoke network`)

Linux: starts Xvfb on :99 unless IRONCALL_E2E_XVFB=0 and a display is available.
Windows: finds an msedgedriver matching the installed WebView2 version (downloading it
into target/e2e/ if needed) and hands it to tauri-driver.
Env: IRONCALL_APP overrides the app binary; IRONCALL_E2E_NATIVE_DRIVER the driver.
"""
import io, os, platform, shutil, subprocess, sys, time, urllib.request, zipfile

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP, WINDOWS  # noqa: E402

SUITES = ["smoke", "workspace", "variables", "network", "responses", "importing", "runner"]
# Run only when named: captures docs/images (`just screenshots`).
EXTRA = ["screenshots_docs"]
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
WEBVIEW2_CLIENT = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"


def fail(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(2)


def webview2_version():
    import winreg

    for hive, key in [(winreg.HKEY_LOCAL_MACHINE, rf"SOFTWARE\WOW6432Node\{WEBVIEW2_CLIENT}"),
                      (winreg.HKEY_LOCAL_MACHINE, rf"SOFTWARE\{WEBVIEW2_CLIENT}"),
                      (winreg.HKEY_CURRENT_USER, rf"Software\{WEBVIEW2_CLIENT}")]:
        try:
            with winreg.OpenKey(hive, key) as k:
                version = winreg.QueryValueEx(k, "pv")[0]
                if version and version != "0.0.0.0":
                    return version
        except OSError:
            pass
    fail("WebView2 runtime not found in the registry")


def driver_version(path):
    out = subprocess.run([path, "--version"], capture_output=True, text=True).stdout
    return next((w for w in out.split() if w[:1].isdigit()), None)


def msedgedriver():
    """An msedgedriver.exe whose version matches WebView2 exactly (WebDriver refuses otherwise)."""
    want = webview2_version()
    given = os.environ.get("IRONCALL_E2E_NATIVE_DRIVER")
    if given:
        have = driver_version(given)
        if have != want:
            fail(f"IRONCALL_E2E_NATIVE_DRIVER is msedgedriver {have}, but WebView2 is {want}")
        return given
    cached = os.path.join(ROOT, "target", "e2e", f"msedgedriver-{want}", "msedgedriver.exe")
    if not os.path.exists(cached):
        arch = {"AMD64": "win64", "ARM64": "arm64", "x86": "win32"}.get(platform.machine(), "win64")
        url = f"https://msedgedriver.microsoft.com/{want}/edgedriver_{arch}.zip"
        print(f"Downloading msedgedriver {want} from {url}")
        try:
            with urllib.request.urlopen(url, timeout=120) as r:
                zipfile.ZipFile(io.BytesIO(r.read())).extract("msedgedriver.exe", os.path.dirname(cached))
        except Exception as e:  # noqa: BLE001
            fail(f"could not download msedgedriver ({e}). Download it for WebView2 {want} from "
                 "https://developer.microsoft.com/microsoft-edge/tools/webdriver/ and set "
                 "IRONCALL_E2E_NATIVE_DRIVER to its path.")
    return cached


def start_xvfb(env):
    if env.get("DISPLAY") and env.get("IRONCALL_E2E_XVFB", "1") != "1":
        return
    if subprocess.run(["pgrep", "-x", "Xvfb"], capture_output=True).returncode != 0:
        subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1400x900x24"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)
    env["DISPLAY"] = ":99"
    env["WAYLAND_DISPLAY"] = ""
    # Wayland desktops often set GDK_BACKEND=wayland, which makes GTK ignore DISPLAY.
    env["GDK_BACKEND"] = "x11"


def main():
    suites = sys.argv[1:] or SUITES
    unknown = [s for s in suites if s not in SUITES + EXTRA]
    if unknown:
        fail(f"unknown suite(s) {unknown}; choose from {SUITES}")
    if not os.path.exists(APP):
        fail(f"app not found at {APP}; build it with `cargo tauri build --debug --no-bundle`")
    if not shutil.which("tauri-driver"):
        fail('tauri-driver not found; install it with `cargo install tauri-driver --version "^2" --locked`')

    env = dict(os.environ)
    if WINDOWS:
        env["IRONCALL_E2E_NATIVE_DRIVER"] = msedgedriver()
    else:
        start_xvfb(env)

    results = {}
    for suite in suites:
        print(f"\n=== {suite} ===", flush=True)
        script = os.path.join(os.path.dirname(__file__), f"{suite}.py")
        results[suite] = subprocess.run([sys.executable, script], env=env).returncode == 0

    print("\n=== summary ===")
    for suite, ok in results.items():
        print(f"{'PASS' if ok else 'FAIL'} {suite}")
    sys.exit(0 if all(results.values()) else 1)


if __name__ == "__main__":
    main()
