#!/usr/bin/env python
"""Performance benchmark for velo, with git as a baseline.

    python sim/team/perf.py <SIMDIR> [--files N] [--history H] [--big-mb M]

Run it on an otherwise idle machine (the workflow runs it between the team rounds and the
analysis). Uses the sandbox's private velo binary (<SIMDIR>/bin/real/velo[.exe]) and the
system `git` when available. Works in <SIMDIR>/perf/work and writes
<SIMDIR>/perf/results.json; prints a compact summary.

Every metric records velo's time in ms (min of repeats for read-only commands), git's time for the
equivalent command when there is one, the budget, and a verdict:
    over-budget   velo > budget
    slow-vs-git   velo > RATIO_LIMIT x git, both above 40 ms
    ok
The BUDGETS table is an initial guess for "smooth and ultra fast"; edit it to taste.
"""
import json, os, random, shutil, statistics, subprocess, sys, threading, time
from pathlib import Path

sim = Path(sys.argv[1]).resolve()

if "--detach" in sys.argv:  # re-launch ourselves fully detached (survives the caller's shell), log to perf/run.log
    (sim / "perf").mkdir(parents=True, exist_ok=True)
    argv = [x for x in sys.argv if x != "--detach"]
    flags = (0x00000008 | 0x00000200) if os.name == "nt" else 0
    with open(sim / "perf" / "run.log", "w") as lf:
        pr = subprocess.Popen([sys.executable, *argv], stdout=lf, stderr=lf, stdin=subprocess.DEVNULL,
                              creationflags=flags, start_new_session=(os.name != "nt"))
    print(f"detached pid {pr.pid}; progress in {sim / 'perf' / 'run.log'}")
    sys.exit(0)


def opt(name, default):
    return int(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default


N_FILES = opt("--files", 2000)
HISTORY = opt("--history", 300)
BIG_MB = opt("--big-mb", 32)
RATIO_LIMIT = 3.0
EXE = ".exe" if os.name == "nt" else ""
VELO = str(sim / "bin" / "real" / f"velo{EXE}")
HAVE_GIT = shutil.which("git") is not None
ENV = dict(os.environ, VELO_AUTHOR_NAME="perf", VELO_AUTHOR_EMAIL="perf@team.test",
           GIT_AUTHOR_NAME="perf", GIT_AUTHOR_EMAIL="perf@team.test",
           GIT_COMMITTER_NAME="perf", GIT_COMMITTER_EMAIL="perf@team.test", GIT_CONFIG_GLOBAL=os.devnull)

# metric -> budget in ms (for the default sizes above)
BUDGETS = {
    "startup --version": 40, "startup status (tiny repo)": 60,
    "init": 100, "save initial (N files)": 3000, "status clean": 150, "status 1 file modified": 200,
    "save 1 file": 200, "save no change": 200, "save 20 files": 400, "diff worktree": 200,
    "diff snapshot..snapshot": 200, "show snapshot": 150, "grep regex": 400, "history default": 100,
    "history --oneline --limit 20": 100, "history --graph --all": 800, "blame (deep history)": 500,
    "history --file (deep history)": 500, "fsck": 3000, "gc": 3000, "squash 10": 1000,
    "undo": 200, "redo": 200, "switch branch (200 files differ)": 500, "merge clean (200+200 files)": 1000,
    "merge with 20 conflicts": 1500, "restore old snapshot": 800, "branches": 100, "tag": 100,
    "stash push+pop": 500, "clone (path)": 2000, "fetch (no change)": 300, "push 50 snapshots": 1500,
    "pull 50 snapshots": 800, "clone (http)": 2500, "bundle create": 2000, "bundle apply": 2000,
    "save big file initial": 3000, "save big file 64KB edit": 800,
    "save latency growth (last50/first50 median)": None,
    "parallel 4 writers (wall, 10 saves each)": 6000, "status while a writer saves": 500,
}
results, notes = [], []


def run(cmd, cwd, timeout=600):
    t = time.perf_counter()
    try:
        r = subprocess.run(cmd, cwd=cwd, env=ENV, capture_output=True, text=True, timeout=timeout,
                           encoding="utf-8", errors="replace", stdin=subprocess.DEVNULL)
        rc, out = r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        rc, out = 124, "TIMEOUT"
    return (time.perf_counter() - t) * 1000, rc, out


def v(cwd, *a, **k):
    return run([VELO, *a], cwd, **k)


def g(cwd, *a, **k):
    return run(["git", "-c", "core.autocrlf=false", "-c", "core.fsmonitor=false", *a], cwd, **k)


def record(name, velo_ms, git_ms=None, rc=0, detail=""):
    b = BUDGETS.get(name)
    verdict = "ok"
    if rc != 0:
        verdict = "error"
    elif b is not None and velo_ms > b:
        verdict = "over-budget"
    elif git_ms and velo_ms > 40 and git_ms > 40 and velo_ms > RATIO_LIMIT * git_ms:
        verdict = "slow-vs-git"
    elif git_ms and velo_ms > 40 and git_ms <= 40 and velo_ms > RATIO_LIMIT * 40:
        verdict = "slow-vs-git"
    results.append({"metric": name, "velo_ms": round(velo_ms, 1), "git_ms": round(git_ms, 1) if git_ms else None,
                    "budget_ms": b, "verdict": verdict, "rc": rc, "detail": detail[:300]})


def best(fn, repeats=3):
    """min wall time of a read-only command; fn returns (ms, rc, out)."""
    runs = [fn() for _ in range(repeats)]
    ms = min(r[0] for r in runs)
    rc = max(r[1] for r in runs)
    return ms, rc, runs[-1][2]


def dirsize(p):
    return sum(f.stat().st_size for f in Path(p).rglob("*") if f.is_file())


# ── workspace ───────────────────────────────────────────────────────────────
work = sim / "perf" / "work"
if work.exists():
    shutil.rmtree(work, ignore_errors=True)
work.mkdir(parents=True)
rnd = random.Random(7)
WORDS = ["alpha", "beta", "gamma", "delta", "inventory", "order", "price", "stock", "warehouse", "report", "tax", "item"]


def gen_file(i):
    lines = rnd.randint(8, 400)
    return "\n".join(" ".join(rnd.choice(WORDS) for _ in range(rnd.randint(3, 12))) + f" {i}.{k}" for k in range(lines)) + "\n"


def make_tree(root):
    for i in range(N_FILES):
        p = root / f"d{i % 50:02d}" / f"s{i % 7}" / f"f{i}.txt"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(gen_file(i), newline="\n")


def edit(root, idxs, tag):
    for i in idxs:
        i %= N_FILES
        p = root / f"d{i % 50:02d}" / f"s{i % 7}" / f"f{i}.txt"
        p.write_text(p.read_text() + f"edit {tag} {i}\n", newline="\n")


# ── startup ─────────────────────────────────────────────────────────────────
tiny = work / "tiny"
tiny.mkdir()
(tiny / "a.txt").write_text("hello\n")
v(tiny, "init"); v(tiny, "save", "init")
ms, rc, _ = best(lambda: v(tiny, "--version"), 7); record("startup --version", ms, None, rc)
ms, rc, _ = best(lambda: v(tiny, "status"), 7)
gms = best(lambda: g(tiny, "status"), 7)[0] if HAVE_GIT and (g(tiny, "init") and g(tiny, "add", "-A") and g(tiny, "commit", "-qm", "i")) else None
record("startup status (tiny repo)", ms, gms, rc)

# ── main repo: N files ──────────────────────────────────────────────────────
main = work / "main"
main.mkdir()
make_tree(main)
tree_bytes = dirsize(main)
notes.append(f"tree: {N_FILES} files, {tree_bytes/1e6:.1f} MB")
ms, rc, _ = v(main, "init"); record("init", ms, g(main, "init")[0] if HAVE_GIT else None, rc)
ms, rc, out = v(main, "save", "initial")
gms = None
if HAVE_GIT:
    gms = g(main, "add", "-A")[0] + g(main, "commit", "-qm", "initial")[0]
record("save initial (N files)", ms, gms, rc, out.strip()[-120:])

ms, rc, _ = best(lambda: v(main, "status")); record("status clean", ms, best(lambda: g(main, "status"))[0] if HAVE_GIT else None, rc)
edit(main, [10], "a")
ms, rc, _ = best(lambda: v(main, "status")); record("status 1 file modified", ms, best(lambda: g(main, "status"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(main, "diff")); record("diff worktree", ms, best(lambda: g(main, "diff"))[0] if HAVE_GIT else None, rc)
ms, rc, out = v(main, "save", "one file"); gms = None
if HAVE_GIT:
    gms = g(main, "commit", "-aqm", "one file")[0]
record("save 1 file", ms, gms, rc, out.strip()[-120:])
ms, rc, _ = v(main, "save", "nothing"); record("save no change", ms, g(main, "commit", "-aqm", "nothing")[0] if HAVE_GIT else None, 0 if rc in (0, 1) else rc)
edit(main, range(100, 120), "b")
ms, rc, _ = v(main, "save", "20 files"); gms = g(main, "commit", "-aqm", "20 files")[0] if HAVE_GIT else None
record("save 20 files", ms, gms, rc)

hist = v(main, "history", "--oneline", "--limit", "5")[2].split()
hashes = [t for t in hist if len(t) >= 16 and all(c in "0123456789abcdef" for c in t)]
if len(hashes) >= 2:
    a, b = hashes[1], hashes[0]
    ms, rc, _ = best(lambda: v(main, "diff", f"{a}..{b}")); record("diff snapshot..snapshot", ms, best(lambda: g(main, "diff", "HEAD~1", "HEAD"))[0] if HAVE_GIT else None, rc)
    ms, rc, _ = best(lambda: v(main, "show", b)); record("show snapshot", ms, best(lambda: g(main, "show", "HEAD"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(main, "grep", "inventory.*stock")); record("grep regex", ms, best(lambda: g(main, "grep", "-E", "inventory.*stock"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(main, "history")); record("history default", ms, best(lambda: g(main, "log"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(main, "branches")); record("branches", ms, best(lambda: g(main, "branch"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = v(main, "tag", "v-perf"); record("tag", ms, g(main, "tag", "v-perf")[0] if HAVE_GIT else None, rc)

# ── deep history on one hot file + latency growth ───────────────────────────
deep = work / "deep"
deep.mkdir()
make_tree(deep)
v(deep, "init"); v(deep, "save", "base")
if HAVE_GIT:
    g(deep, "init"); g(deep, "add", "-A"); g(deep, "commit", "-qm", "base")
save_ms, git_save_ms = [], []
hot = deep / "d00" / "s0" / "f0.txt"
for k in range(HISTORY):
    edit(deep, [0, (k * 37) % N_FILES], f"h{k}")
    ms, rc, _ = v(deep, "save", f"change {k}")
    save_ms.append(ms)
    if HAVE_GIT:
        git_save_ms.append(g(deep, "commit", "-aqm", f"change {k}")[0])
if len(save_ms) >= 100:
    first, last = statistics.median(save_ms[:50]), statistics.median(save_ms[-50:])
    ratio = last / first if first else 0
    results.append({"metric": "save latency growth (last50/first50 median)", "velo_ms": round(last, 1),
                    "git_ms": round(statistics.median(git_save_ms[-50:]), 1) if git_save_ms else None, "budget_ms": None,
                    "verdict": "over-budget" if ratio > 1.5 else "ok", "rc": 0,
                    "detail": f"first50 median {first:.0f} ms, last50 median {last:.0f} ms, ratio {ratio:.2f} (limit 1.5); p95 {sorted(save_ms)[int(len(save_ms)*.95)]:.0f} ms"})
ms, rc, _ = best(lambda: v(deep, "history", "--oneline", "--limit", "20")); record("history --oneline --limit 20", ms, best(lambda: g(deep, "log", "--oneline", "-20"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(deep, "history", "--graph", "--all")); record("history --graph --all", ms, best(lambda: g(deep, "log", "--graph", "--all", "--oneline"))[0] if HAVE_GIT else None, rc)
ms, rc, _ = best(lambda: v(deep, "blame", "d00/s0/f0.txt")); record("blame (deep history)", ms, best(lambda: g(deep, "blame", "d00/s0/f0.txt"))[0] if HAVE_GIT else None, rc, f"{HISTORY} snapshots touch this file")
ms, rc, _ = best(lambda: v(deep, "history", "--file", "d00/s0/f0.txt", "--oneline")); record("history --file (deep history)", ms, best(lambda: g(deep, "log", "--oneline", "--", "d00/s0/f0.txt"))[0] if HAVE_GIT else None, rc)
ms, rc, out = v(deep, "fsck", timeout=900); record("fsck", ms, g(deep, "fsck")[0] if HAVE_GIT else None, rc, out.strip()[-120:])
ms, rc, _ = v(deep, "undo"); record("undo", ms, None, rc)
ms, rc, _ = v(deep, "redo"); record("redo", ms, None, rc)
edit(deep, [5, 6], "sq"); v(deep, "save", "pre-squash")
ms, rc, out = v(deep, "squash", "10", "squashed"); record("squash 10", ms, None, rc, out.strip()[-160:])
ms, rc, out = v(deep, "gc", timeout=900); record("gc", ms, g(deep, "gc", "-q")[0] if HAVE_GIT else None, rc, out.strip()[-120:])
old = v(deep, "history", "--oneline", "--limit", "100")[2].split()
old = [t for t in old if len(t) >= 16 and all(c in "0123456789abcdef" for c in t)]
if len(old) > 50:
    ms, rc, _ = v(deep, "restore", old[50], "--force"); record("restore old snapshot", ms, None, rc)
    v(deep, "switch", "main", "--force")
vs, gs = dirsize(deep / ".velo"), (dirsize(deep / ".git") if HAVE_GIT else None)
notes.append(f"storage after {HISTORY} snapshots: .velo {vs/1e6:.1f} MB" + (f", .git {gs/1e6:.1f} MB" if gs else "") + f" (tree {tree_bytes/1e6:.1f} MB)")

# ── branches / merge ────────────────────────────────────────────────────────
br = work / "branching"
br.mkdir()
make_tree(br)
v(br, "init"); v(br, "save", "base")
if HAVE_GIT:
    g(br, "init", "-q", "-b", "main"); g(br, "add", "-A"); g(br, "commit", "-qm", "base")
v(br, "switch", "feature"); edit(br, range(0, 200), "f"); v(br, "save", "feature side")
if HAVE_GIT:
    g(br, "checkout", "-qb", "feature"); g(br, "commit", "-aqm", "feature side")
v(br, "switch", "main")
if HAVE_GIT:
    g(br, "checkout", "-q", "main")
ms, rc, _ = v(br, "switch", "feature"); gms = g(br, "checkout", "-q", "feature")[0] if HAVE_GIT else None
record("switch branch (200 files differ)", ms, gms, rc)
v(br, "switch", "main")
edit(br, range(1000, 1200), "m"); v(br, "save", "main side")
gm = None
if HAVE_GIT:
    g(br, "checkout", "-q", "main"); g(br, "commit", "-aqm", "main side")
    gm = g(br, "merge", "-q", "--no-edit", "feature")[0]
ms, rc, out = v(br, "merge", "feature"); record("merge clean (200+200 files)", ms, gm, rc, out.strip()[-120:])
v(br, "save", "merged")
cf = work / "conflicts"
cf.mkdir()
make_tree(cf)
v(cf, "init"); v(cf, "save", "base")
v(cf, "switch", "other")
for i in range(20):
    p = cf / f"d{i % 50:02d}" / f"s{i % 7}" / f"f{i}.txt"
    p.write_text("OTHER\n" + p.read_text(), newline="\n")
v(cf, "save", "other")
v(cf, "switch", "main")
for i in range(20):
    p = cf / f"d{i % 50:02d}" / f"s{i % 7}" / f"f{i}.txt"
    p.write_text("MAIN\n" + p.read_text().split("\n", 1)[0] + "\n", newline="\n")
v(cf, "save", "main")
ms, rc, out = v(cf, "merge", "other"); record("merge with 20 conflicts", ms, None, 0, out.strip()[-160:])
v(cf, "merge", "--abort")
(cf / "S.txt").write_text("stash me\n")
ms, rc, _ = v(cf, "stash", "push", "s1"); ms2, rc2, _ = v(cf, "stash", "pop", "s1")
record("stash push+pop", ms + ms2, None, max(rc, rc2))

# ── sync: clone / push / pull / http / bundle ───────────────────────────────
sy = work / "sync"
sy.mkdir()
ms, rc, out = v(sy, "clone", str(deep), "c1"); record("clone (path)", ms, g(sy, "clone", "-q", str(deep), "g1")[0] if HAVE_GIT else None, rc, out.strip()[-120:])
c1, c2 = sy / "c1", None
v(sy, "clone", str(deep), "c2")
c2 = sy / "c2"
for k in range(50):
    edit(c1, [k * 3], f"p{k}"); v(c1, "save", f"push {k}")
ms, rc, out = v(c1, "push"); record("push 50 snapshots", ms, None, rc, out.strip()[-120:])
v(c2, "fetch")
ms, rc, _ = best(lambda: v(c2, "fetch")); record("fetch (no change)", ms, None, rc)
ms, rc, out = v(c2, "pull"); record("pull 50 snapshots", ms, None, rc, out.strip()[-120:])
port = 8499
srv = subprocess.Popen([VELO, "serve-http", str(deep), "--listen", f"127.0.0.1:{port}"], env=ENV,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.0)
ms, rc, out = v(sy, "clone", f"http://127.0.0.1:{port}/", "ch"); record("clone (http)", ms, None, rc, out.strip()[-120:])
srv.terminate()
bf = sim / "perf" / "deep.bundle"
ms, rc, out = v(deep, "bundle", "create", str(bf)); record("bundle create", ms, g(deep, "bundle", "create", str(sim / "perf" / "deep.gitbundle"), "--all")[0] if HAVE_GIT else None, rc)
nb = work / "bundle-target"; nb.mkdir(); v(nb, "init")
ms, rc, out = v(nb, "bundle", "apply", str(bf)); record("bundle apply", ms, None, rc, out.strip()[-120:])

# ── big file ────────────────────────────────────────────────────────────────
bg = work / "bigfile"
bg.mkdir()
big = bg / "blob.bin"
big.write_bytes(random.Random(3).randbytes(BIG_MB * 1024 * 1024))
v(bg, "init")
ms, rc, out = v(bg, "save", "blob"); gms = None
if HAVE_GIT:
    g(bg, "init"); g(bg, "add", "-A"); gms = g(bg, "commit", "-qm", "blob")[0]
record("save big file initial", ms, gms, rc, f"{BIG_MB} MB incompressible")
data = bytearray(big.read_bytes()); mid = len(data) // 2
data[mid:mid + 65536] = random.Random(4).randbytes(65536)
big.write_bytes(bytes(data))
before = dirsize(bg / ".velo")
ms, rc, out = v(bg, "save", "edit 64KB"); gms = g(bg, "commit", "-aqm", "edit")[0] if HAVE_GIT else None
growth = dirsize(bg / ".velo") - before
record("save big file 64KB edit", ms, gms, rc, f"store grew {growth/1e6:.2f} MB for a 64 KB edit of a {BIG_MB} MB file")
if growth > 0.5 * BIG_MB * 1024 * 1024:
    notes.append(f"STORAGE: a 64 KB edit of a {BIG_MB} MB file grew the store by {growth/1e6:.1f} MB (no chunk-level dedup?)")

# ── concurrency ─────────────────────────────────────────────────────────────
cc = work / "concurrent"
cc.mkdir()
make_tree(cc)
v(cc, "init"); v(cc, "save", "base")
shared = work / "shared"
shutil.copytree(cc, shared)
for i in range(4):
    v(work, "clone", str(shared), f"cw{i}")
errs, lock, retries = [], threading.Lock(), [0]


def writer(i):
    d = work / f"cw{i}"
    for k in range(10):
        edit(d, [i * 10 + k], f"w{i}k{k}")
        _, rc, out = v(d, "save", f"w{i} {k}")
        if rc != 0:
            with lock:
                errs.append(f"w{i} save: {out.strip()[-150:]}")
        for _try in range(15):
            _, rc, out = v(d, "push")
            if rc == 0:
                break
            with lock:
                retries[0] += 1
            v(d, "pull")
            v(d, "merge", "origin/main"); v(d, "save", "merge")
        else:
            with lock:
                errs.append(f"w{i} push never succeeded: {out.strip()[-150:]}")


t0 = time.perf_counter()
ths = [threading.Thread(target=writer, args=(i,)) for i in range(4)]
[t.start() for t in ths]; [t.join() for t in ths]
record("parallel 4 writers (wall, 10 saves each)", (time.perf_counter() - t0) * 1000, None, 0,
       f"{retries[0]} refused pushes needed pull+merge+retry (expected: fast-forward-only); {len(errs)} errors: " + " | ".join(errs[:3]))
if errs:
    notes.append(f"CONCURRENCY: {len(errs)} error(s) with 4 parallel writers, e.g. {errs[0]}")

stop = threading.Event()


def writer2():
    d = work / "cw0"
    k = 0
    while not stop.is_set():
        edit(d, [k % 100], f"x{k}"); v(d, "save", f"bg {k}"); k += 1


th = threading.Thread(target=writer2); th.start()
time.sleep(0.5)
ms, rc, out = best(lambda: v(work / "cw1", "status"), 5)
stop.set(); th.join()
record("status while a writer saves", ms, None, rc, out.strip()[-120:])

# ── output ──────────────────────────────────────────────────────────────────
bad = [r for r in results if r["verdict"] != "ok"]
summary = {"files": N_FILES, "history": HISTORY, "bigMB": BIG_MB, "git": HAVE_GIT, "notes": notes,
           "metrics": results, "flagged": [r["metric"] + " [" + r["verdict"] + "]" for r in bad]}
(sim / "perf").mkdir(exist_ok=True)
(sim / "perf" / "results.json").write_text(json.dumps(summary, indent=2))
print(f"{len(results)} metrics, {len(bad)} flagged; results in {sim / 'perf' / 'results.json'}")
for r in results:
    print(f"{r['verdict']:12} {r['metric']:48} velo {r['velo_ms']:>9.1f} ms  git {str(r['git_ms']):>8}  budget {r['budget_ms']}")
for n in notes:
    print("NOTE", n)
