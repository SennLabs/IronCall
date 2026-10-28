"""Idle memory budget (PLAN.md Phase 8, ADR 0008): PSS summed over IronCall's process tree,
excluding GPU driver libraries, which vary with the machine's graphics hardware.

Usage: python3 tests/perf/idle_memory.py [APP] [RUNS]   (default target/release/ironcall, 3)
Run on a real display with a release build (`cargo tauri build --no-bundle`). Each run
starts the app with an empty temporary data folder, waits 10s, and measures."""
import os, re, subprocess, sys, tempfile, time

BUDGET_MB = 200
# Mesa, NVIDIA and libglvnd libraries, and the libraries only they pull in.
GPU = re.compile(r"/lib(nvidia|EGL|GLX|GLdispatch|gallium|LLVM|drm|vulkan|gbm|glapi|elf-|sensors|pciaccess)"
                 r"|/dri/|_dri\.so")

app = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "../../target/release/ironcall")
runs = int(sys.argv[2]) if len(sys.argv) > 2 else 3


def tree(root):
    kids = {}
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                ppid = int(open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()[1])
                kids.setdefault(ppid, []).append(int(p))
            except OSError:
                pass
    out, todo = [], [root]
    while todo:
        p = todo.pop()
        out.append(p)
        todo += kids.get(p, [])
    return out


def measure(pid):
    total = gpu = 0.0
    for p in tree(pid):
        try:
            mapping = ""
            for line in open(f"/proc/{p}/smaps"):
                f = line.split()
                if "-" in f[0] and not f[0].endswith(":"):
                    mapping = f[5] if len(f) > 5 else ""
                elif f[0] == "Pss:":
                    mb = int(f[1]) / 1024
                    total += mb
                    if GPU.search(mapping):
                        gpu += mb
        except OSError:
            pass
    return total, gpu


worst = 0.0
for i in range(runs):
    data = tempfile.mkdtemp(prefix="ironcall-mem-")
    proc = subprocess.Popen([app], env={**os.environ, "IRONCALL_DATA_DIR": data, "XDG_DATA_HOME": data},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(10)
        total, gpu = measure(proc.pid)
    finally:
        proc.terminate()
        proc.wait(timeout=10)
    worst = max(worst, total - gpu)
    print(f"run {i + 1}: {total - gpu:.0f}MB ({total:.0f}MB total, {gpu:.0f}MB GPU driver libraries)")

ok = worst < BUDGET_MB
print(f"{'PASS' if ok else 'FAIL'} idle memory {worst:.0f}MB (< {BUDGET_MB}MB, GPU driver libraries excluded)")
sys.exit(0 if ok else 1)
