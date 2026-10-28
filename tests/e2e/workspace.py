"""Phase 2 E2E: open a workspace, edit and save, external changes, conflicts,
error nodes and session restore, against the real app. Uses a temporary
app data dir so the user's own app data is never touched.

Also the Phase 8 budget "a tree with 2,000+ requests stays smooth": open, scroll and
filter a 2,000-request workspace (enforced with IRONCALL_E2E_PERF=1 on a release build)."""
import json, os, sys, tempfile, time

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, CTRL, ENFORCE_PERF, RELEASE, Driver, data_env, restore_workspace  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "ws")
os.makedirs(os.path.join(data, APP_ID))
os.makedirs(ws)

REQ_ID = "01J9Z3K7T2V8Q4M6N0P5R1S3W7"


def request_yaml(url):
    return f"""schema: 'ironcall/request@1'
id: '{REQ_ID}'
name: 'Get user'
method: 'GET'
url: '{url}'
"""


open(os.path.join(ws, "ironcall.yaml"), "w").write(
    "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8J9'\nname: 'E2E'\n")
req_path = os.path.join(ws, "get-user.yaml")
open(req_path, "w").write(request_yaml("https://api.example.com/users"))
broken = "schema: 'ironcall/request@1'\n<<<<<<< HEAD\nname: 'a'\n=======\nname: 'b'\n>>>>>>> theirs\n"
broken_path = os.path.join(ws, "broken.yaml")
open(broken_path, "w").write(broken)
restore_workspace(data, ws)

d = Driver(OUT, env=data_env(data))
try:
    item = d.find(css='[role="treeitem"][aria-label="Get user"]')
    d.check(True, "workspace restored from session and request shown in tree")
    d.check(d.exists(css='[role="treeitem"][aria-label="broken (error)"], [role="treeitem"][data-kind="error"]'),
            "merge-conflict file shown as an error node")
    d.shot("ws-1-tree")

    # Script in the webview can't open (and so grant itself) a folder the user didn't pick,
    # or read the app data folder (history, cookies, settings) through a body command.
    invoke = """
        const done = arguments[arguments.length - 1];
        window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
          .then(() => done('allowed'), (e) => done('refused: ' + JSON.stringify(e)));
    """
    unpicked = os.path.join(tmp, "unpicked")
    os.makedirs(unpicked)
    open(os.path.join(unpicked, "ironcall.yaml"), "w").write(
        "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8K1'\nname: 'Unpicked'\n")
    result = d.execute_async(invoke, ("workspace_open", {"path": unpicked}))
    d.check(result.startswith("refused"), f"workspace_open refuses a folder that wasn't picked ({result})")
    result = d.execute_async(invoke, ("open_body", {"path": os.path.join(data, APP_ID, "session.json")}))
    d.check(result.startswith("refused"), f"open_body refuses files in the app data folder ({result})")

    # Open, edit, save with Ctrl+S.
    d.click(item)
    url_css = 'input[aria-label="URL"]'
    d.wait(lambda: d.prop(d.find(css=url_css), "value") == "https://api.example.com/users")
    d.type_into("/v2", css=url_css)
    d.type_into(CTRL + "s" + RELEASE, css=url_css)
    saved = d.wait(lambda: "users/v2" in open(req_path).read())
    # Wait until the tab itself is clean (the save round-trip has finished): an external
    # change that lands while our own save is in flight is treated as a conflict.
    d.wait(lambda: d.execute("const t = document.querySelector('[role=\"tab\"][aria-selected=\"true\"]'); return !!t && !/unsaved|saving/i.test(t.getAttribute('aria-label') || '');"), timeout=10)
    time.sleep(0.5)
    d.check(saved, "Ctrl+S wrote the change to disk")
    content = open(req_path).read()
    d.check(content.startswith("schema: 'ironcall/request@1'\nid: '" + REQ_ID + "'"), "saved in canonical form, ID kept")
    d.check(open(broken_path).read() == broken, "error-node file left untouched")

    # External edit while the tab is clean: reloads silently.
    open(req_path, "w").write(request_yaml("https://api.example.com/external"))
    reloaded = d.wait(lambda: d.prop(d.find(css=url_css), "value") == "https://api.example.com/external")
    d.check(reloaded, "clean tab reloaded after an external edit")

    # External edit while dirty: conflict bar, keep mine.
    d.type_into("/mine", css=url_css)
    time.sleep(0.5)
    open(req_path, "w").write(request_yaml("https://api.example.com/theirs"))
    bar = d.wait(lambda: d.exists(css='[aria-label="Conflict"]', timeout=1))
    d.shot("ws-2-conflict")
    d.check(bar, "dirty tab shows a conflict bar after an external edit")
    if bar:
        d.click_on(xpath='//*[@aria-label="Conflict"]//button[normalize-space()="Keep mine"]')
        kept = d.wait(lambda: "external/mine" in open(req_path).read())
        d.check(kept, "Keep mine force-saved our version")

    # Session restore: restart the app and expect the tab back.
    time.sleep(1.5)  # session autosave is debounced
    d.stop_session()
    d.start()
    restored = d.wait(lambda: d.exists(css='[role="tab"][aria-label^="Get user"]', timeout=1), timeout=15)
    d.shot("ws-3-restored")
    d.check(restored, "open tab restored after restart")

    # --- 2,000 requests: 1,500 at the top level (all rows visible) + 20 folders of 25.
    big = os.path.join(tmp, "big")
    os.makedirs(big)
    open(os.path.join(big, "ironcall.yaml"), "w").write(
        "schema: 'ironcall/collection@1'\nid: '01J9Z3M0A1B2C3D4E5F6G7H8JA'\nname: 'Big'\n")

    def write_request(folder, n):
        open(os.path.join(folder, f"req-{n:04d}.yaml"), "w").write(
            f"schema: 'ironcall/request@1'\nid: '01JA000000000000000{n:07d}'\n"
            f"name: 'Request {n:04d}'\nmethod: 'GET'\nurl: 'https://api.example.com/items/{n}'\n")

    for n in range(1500):
        write_request(big, n)
    for f in range(20):
        sub = os.path.join(big, f"folder-{f:02d}")
        os.makedirs(sub)
        for n in range(1500 + f * 25, 1525 + f * 25):
            write_request(sub, n)
    d.stop_session()
    restore_workspace(data, big)
    t0 = time.time()
    d.start()
    shown = d.wait(lambda: d.exists(css='[role="treeitem"][aria-label="Request 0000"]', timeout=1), timeout=30)
    opened = time.time() - t0
    d.check(shown, f"2,000-request workspace restored and shown ({opened:.1f}s from launch)")

    scroll_gap = d.execute_async("""
        const done = arguments[arguments.length - 1];
        const el = document.querySelector('[role="tree"]');
        let last = performance.now(), worst = 0, n = 0;
        function step(t) {
          worst = Math.max(worst, t - last); last = t;
          el.scrollTop += 600; el.dispatchEvent(new Event('scroll'));
          if (++n < 120) requestAnimationFrame(step); else done(worst);
        }
        requestAnimationFrame((t) => { last = t; requestAnimationFrame(step); });
    """)
    # Typing into the filter, one character per frame: worst frame gap while it re-filters.
    filter_gap = d.execute_async("""
        const done = arguments[arguments.length - 1];
        const input = document.querySelector('input[aria-label="Filter requests"]');
        const text = 'request 1999';
        let last = performance.now(), worst = 0, i = 0;
        function step(t) {
          worst = Math.max(worst, t - last); last = t;
          if (i < text.length) {
            input.value = text.slice(0, ++i);
            input.dispatchEvent(new Event('input', {bubbles: true}));
            requestAnimationFrame(step);
          } else {
            requestAnimationFrame((t2) => done(Math.max(worst, t2 - last)));
          }
        }
        requestAnimationFrame((t) => { last = t; requestAnimationFrame(step); });
    """)
    filtered = d.wait(lambda: d.exists(css='[role="treeitem"][aria-label="Request 1999"]', timeout=1), timeout=5)
    d.check(filtered, "filter finds a request inside a folder")
    for label, gap in [("scrolling the 2,000-request tree", scroll_gap),
                       ("filtering the 2,000-request tree", filter_gap)]:
        if ENFORCE_PERF:
            d.check(gap is not None and gap < 50, f"worst frame gap {label}: {gap:.0f}ms (< 50ms)")
        else:
            print(f"INFO worst frame gap {label}: {gap:.0f}ms (budget 50ms; enforced with IRONCALL_E2E_PERF=1 on a release build)")
    d.shot("ws-4-big-tree")
except Exception:
    d.shot("ws-failure")
    raise
finally:
    d.quit()
sys.exit(0 if d.ok else 1)
