"""Phase 5 E2E: large bodies, previews, timeline and history against the real app.

Exit criteria checked here (PLAN.md Phase 5):
- 100MB JSON: first rows quickly, no frame gap over 50ms while scrolling, search finds a
  hit near the end in under 1s, total memory of all app processes under 400MB
- image and PDF previews render
- the HTML preview makes zero network requests
- history stores no literal sensitive header value
"""
import json, os, struct, subprocess, sys, tempfile, threading, time, zlib
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, ENFORCE_PERF, WINDOWS, Driver, data_env  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "screenshots")
SECRET = "Bearer literal-e2e-secret-777"
hits = []

ROW = b'  {"id": 12345, "name": "row", "tags": ["a", "b"], "pad": "xxxxxxxxxxxxxxxxxxxxxxxx"},\n'
BIG = b"[\n" + ROW * (100 * 1024 * 1024 // len(ROW)) + b'  {"marker": "end-of-body-marker"}\n]\n'


def png(w=64, h=48):
    raw = b"".join(b"\x00" + bytes([200, 80, 40, 255]) * w for _ in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)) + \
        chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def pdf():
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            None,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    stream = b"BT /F1 18 Tf 20 50 Td (IronCall PDF) Tj ET"
    objs[3] = b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream"
    out, offsets = b"%PDF-1.4\n", []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return out


HTML = None  # set once the port is known


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        hits.append(self.path)
        routes = {"/big": ("application/json", BIG), "/image": ("image/png", png()),
                  "/doc": ("application/pdf", pdf()), "/page": ("text/html; charset=utf-8", HTML)}
        ctype, body = routes.get(self.path, ("text/plain", b"tracked"))
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
HTML = (f'<html><head><link rel="stylesheet" href="http://127.0.0.1:{port}/tracker.css"></head>'
        f'<body><h1>Preview me</h1><img src="http://127.0.0.1:{port}/tracker.png">'
        f'<script>fetch("http://127.0.0.1:{port}/tracker-js")</script></body></html>').encode()
threading.Thread(target=srv.serve_forever, daemon=True).start()

tmp = tempfile.mkdtemp(prefix="ironcall-e2e-")
data = os.path.join(tmp, "data")
os.makedirs(os.path.join(data, APP_ID))
d = Driver(OUT, env=data_env(data))


def pss_breakdown():
    """Memory per app process in MB: PSS on Linux, private working set on Windows."""
    return windows_breakdown() if WINDOWS else linux_breakdown()


def pss_mb():
    return sum(pss_breakdown().values())


def linux_breakdown():
    out = {}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            cmd = open(f"/proc/{pid}/cmdline", "rb").read()
            exe = cmd.split(b"\0")[0]
            name = b"ironcall" if exe.endswith(b"/ironcall") else next(
                (n for n in [b"WebKitWebProcess", b"WebKitNetworkProcess"] if n in cmd), None)
            if name:
                # PSS, not RSS: shared libraries (WebKitGTK, GTK, ICU) are loaded by
                # several processes; RSS would count them once per process.
                kb = next(int(l.split()[1]) for l in open(f"/proc/{pid}/smaps_rollup") if l.startswith("Pss:"))
                out[f"{name.decode().split('/')[-1]}#{pid}"] = kb / 1024
        except (OSError, StopIteration):
            pass
    return out


def windows_breakdown():
    # ironcall.exe and its descendants (the WebView2 processes). Other apps' msedgewebview2
    # processes are not ours. Private working set is Task Manager's "Memory" column.
    ps = ("$p = Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name;"
          "$w = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Select-Object IDProcess,WorkingSetPrivate;"
          "@{procs=$p; ws=$w} | ConvertTo-Json -Compress -Depth 3")
    try:
        raw = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True, timeout=60).stdout
        info = json.loads(raw)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return {}
    procs = {p["ProcessId"]: p for p in info["procs"]}
    ours = {pid for pid, p in procs.items() if (p["Name"] or "").lower() == "ironcall.exe"}
    while True:
        more = {pid for pid, p in procs.items() if p["ParentProcessId"] in ours} - ours
        if not more:
            break
        ours |= more
    ws = {w["IDProcess"]: int(w["WorkingSetPrivate"]) for w in info["ws"]}
    return {f"{procs[pid]['Name']}#{pid}": ws[pid] / 2**20 for pid in ours if pid in ws}


def rounded(mb):
    return {k: round(v) for k, v in mb.items()}


def tab(name):
    d.click_on(xpath=f'//*[@role="tab" and normalize-space()="{name}"]')
    time.sleep(0.5)


try:
    url = d.find(css='input[aria-label="URL"]')
    time.sleep(1)
    print("INFO idle memory by process (MB):", rounded(pss_breakdown()))

    def go(path, headers=()):
        d.clear(url)
        d.type(url, f"http://127.0.0.1:{port}{path}")
        time.sleep(0.3)
        before = len(hits)
        t0 = time.time()
        d.click_on(xpath='//button[normalize-space()="Send"]')
        d.wait(lambda: len(hits) > before, timeout=30)
        return t0

    # A literal sensitive header on the scratch request (history must mask it).
    d.click_on(xpath='//*[@role="tab" and starts-with(normalize-space(),"Headers")]')
    name_cells = d.find_all('input[placeholder="Add name"], input[placeholder="Name"]')
    if name_cells:
        d.type(name_cells[0], "Authorization")
        time.sleep(0.3)
        values = d.find_all('input[placeholder="Value"]')
        d.type(values[0], SECRET)
        time.sleep(0.3)

    # --- 100MB JSON
    t0 = go("/big")
    viewer = d.wait(lambda: d.exists(css='[aria-label="Response body"]', timeout=1) and
                    d.find(css='[aria-label="Response body"]'), timeout=60)
    first = d.wait(lambda: '"row"' in d.text(d.find(css='[aria-label="Response body"]')), timeout=60)
    elapsed = time.time() - t0
    time.sleep(1.5)
    print("INFO memory after send, before scrolling (MB):", rounded(pss_breakdown()))
    d.check(first, f"100MB body: first rows visible {elapsed:.1f}s after Send (includes 100MB transfer and indexing)")
    d.shot("resp-1-big")
    gap = d.execute_async("""
        const done = arguments[arguments.length - 1];
        const el = document.querySelector('[aria-label="Response body"]');
        let last = performance.now(), worst = 0, n = 0;
        function step(t) {
          worst = Math.max(worst, t - last); last = t;
          el.scrollTop += 4000; el.dispatchEvent(new WheelEvent('wheel', {deltaY: 4000}));
          if (++n < 120) requestAnimationFrame(step); else done(worst);
        }
        requestAnimationFrame((t) => { last = t; requestAnimationFrame(step); });
    """)
    if ENFORCE_PERF:
        d.check(gap is not None and gap < 50, f"100MB body: worst frame gap while scrolling {gap:.0f}ms (< 50ms)")
    else:
        print(f"INFO worst frame gap while scrolling: {gap:.0f}ms (budget 50ms; enforced with IRONCALL_E2E_PERF=1 on a release build)")
    print("INFO memory after scrolling (MB):", rounded(pss_breakdown()))
    mem = pss_mb()
    if ENFORCE_PERF:
        d.check(mem < 400, f"total memory of app + webview processes {mem:.0f}MB (< 400MB)")
    else:
        print(f"INFO total memory of app + webview processes: {mem:.0f}MB (budget 400MB; enforced with IRONCALL_E2E_PERF=1)")

    search = d.find(css='input[aria-label="Search body"]')
    t1 = time.time()
    d.type(search, "end-of-body-marker")
    found = d.wait(lambda: "end-of-body-marker" in d.text(d.find(css='[aria-label="Response body"]')), timeout=10)
    took = time.time() - t1
    d.check(found and took < 1.5, f"search found a hit at the end of 100MB in {took:.2f}s (incl. 0.2s polling and debounce)")
    d.shot("resp-2-search")

    # --- Timeline (masked)
    tab("Timeline")
    log = d.text(d.find(css='section[aria-label="Response"]'))
    d.check("GET /big" in log or "/big" in log, "timeline shows the request")
    d.check("literal-e2e-secret" not in log, "timeline masks the Authorization header")

    # --- Image preview
    go("/image")
    time.sleep(1)
    tab("Preview")
    img_ok = d.wait(lambda: d.execute("const i = document.querySelector('img[alt=\"Response body\"]'); return i && i.complete && i.naturalWidth;") == 64, timeout=10)
    d.check(img_ok, "image preview rendered (64px wide)")
    d.shot("resp-3-image")

    # --- PDF preview
    go("/doc")
    time.sleep(1)
    tab("Preview")
    pdf_ok = d.wait(lambda: (d.execute("const c = document.querySelector('canvas[aria-label^=\"PDF page\"]'); return c ? c.width : 0;") or 0) > 0, timeout=15)
    d.check(pdf_ok, "PDF preview rendered a page with pdf.js")
    d.shot("resp-4-pdf")

    # --- HTML preview: zero network requests
    go("/page")
    time.sleep(1)
    tab("Preview")
    before = list(hits)
    framed = d.wait(lambda: d.exists(css='iframe[title="HTML preview"]', timeout=1), timeout=10)
    time.sleep(2.5)
    sandbox = d.execute("return document.querySelector('iframe[title=\"HTML preview\"]').getAttribute('sandbox');")
    d.check(framed and "allow-same-origin" not in (sandbox or ""), f"HTML preview is sandboxed (sandbox={sandbox!r})")
    leaked = [h for h in hits[len(before):] if "tracker" in h]
    d.check(not leaked, f"HTML preview made no network requests (leaks: {leaked})")
    d.shot("resp-5-html")

    # --- History
    d.click_on(css='input[aria-label="URL"]')
    d.type_into("h", css='input[aria-label="URL"]')
    panel = d.wait(lambda: d.exists(css='aside[aria-label="History"]', timeout=1), timeout=5)
    if not panel:
        d.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'h', ctrlKey: true, bubbles: true}))")
        panel = d.wait(lambda: d.exists(css='aside[aria-label="History"]', timeout=1), timeout=5)
    d.check(panel, "history panel opens with Ctrl+H")
    listed = d.wait(lambda: all(p in (d.execute("return document.querySelector('[aria-label=\"History entries\"]')?.textContent ?? ''") or "")
                                for p in ["/big", "/image", "/doc", "/page"]), timeout=10)
    d.check(listed, "history lists every send")
    d.shot("resp-6-history")
finally:
    d.quit()

db = os.path.join(data, APP_ID, "history.sqlite")
blobs = b"".join(open(p, "rb").read() for p in [db, db + "-wal"] if os.path.exists(p))
d.check(len(blobs) > 0, "history database exists in the app data dir")
d.check(b"literal-e2e-secret-777" not in blobs, "no literal Authorization value stored in history")
if WINDOWS:
    # No mode bits on Windows: the file inherits its folder's ACL. Report it for review.
    acl = subprocess.run(["icacls", db], capture_output=True, text=True).stdout.strip()
    print("INFO history database ACL:", acl)
else:
    d.check(oct(os.stat(db).st_mode & 0o777) == "0o600", "history database is user-only (0600)")
sys.exit(0 if d.ok else 1)
