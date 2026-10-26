#!/usr/bin/env python3
"""Scale benchmark for velody: many files, long histories.

Drives the real release CLI (no library calls), so the numbers are what a
user sees. Standard library only. See bench/README.md for method and caveats.
"""
import argparse
import json
import os
import platform
import random
import re
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PRESETS = {
    "small": (1_000, 50),
    "envelope-a": (10_000, 2_000),
    "envelope-b": (100_000, 200),
    "ratio-100k": (100_000, 500),
}
FILES_PER_DIR = 100
FILE_BYTES = 200
EXE = ".exe" if os.name == "nt" else ""
# Per-command wall-clock limit in seconds; None means no limit. Set by
# --command-timeout so a hung velody process cannot stall a detached run.
COMMAND_TIMEOUT = None


def run(velody, args, cwd, check=True):
    """Run one velody command; return (seconds, stdout)."""
    t0 = time.perf_counter()
    try:
        p = subprocess.run([str(velody), *args], cwd=cwd, capture_output=True,
                           text=True, encoding="utf-8", errors="replace",
                           timeout=COMMAND_TIMEOUT)
    except subprocess.TimeoutExpired:
        sys.exit(f"velody {' '.join(args)} exceeded --command-timeout {COMMAND_TIMEOUT}s")
    dt = time.perf_counter() - t0
    if check and p.returncode != 0:
        sys.exit(f"velody {' '.join(args)} failed ({p.returncode}):\n{p.stdout}\n{p.stderr}")
    return dt, p.stdout


def content(rng, path, version):
    head = f"{path} v{version}\n"
    body = "".join(rng.choice("abcdefghijklmnopqrstuvwxyz0123456789 ")
                   for _ in range(FILE_BYTES - len(head) - 1))
    return (head + body + "\n").encode()


def rel(i):
    d = i // FILES_PER_DIR
    return f"d{d // 10:03d}/s{d % 10}/f{i:06d}.txt"


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(q * (len(xs) - 1))))]


def summarize(xs):
    return {"n": len(xs), "median_s": statistics.median(xs), "p95_s": pct(xs, 0.95)}


def dir_size(p):
    return sum(f.stat().st_size for f in Path(p).rglob("*") if f.is_file())


def median_of(n, velody, args, cwd):
    return statistics.median(run(velody, args, cwd)[0] for _ in range(n))


def cpu_name():
    if platform.system() == "Linux":
        try:
            for line in open("/proc/cpuinfo"):
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
        except OSError:
            pass
    return platform.processor() or "unknown"


def history0_ratio(saves):
    """One-file-save ratio against the first one-file saves.

    saves[0] is the initial full save and is excluded. history-0 is the
    median of the next 10 saves; the tail is the median of the last 10% of
    saves. Returns (history0_median_s, ratio), or (None, None) when there
    are too few saves to separate the two.
    """
    one_file = saves[1:]
    if len(one_file) < 2:
        return None, None
    base = statistics.median(one_file[:10])
    k = max(1, len(saves) // 10)
    return base, statistics.median(saves[-k:]) / base


def machine_info(defender_note=None):
    """Identify the machine that produced a result."""
    m = {
        "os": platform.system(),
        "platform": platform.platform(),
        "cpu": cpu_name(),
        "logical_cores": os.cpu_count(),
    }
    if os.name == "nt":
        m["defender_note"] = defender_note or "not provided: assume real-time scanning is ON"
    elif defender_note:
        m["defender_note"] = defender_note
    return m


def write_partial(out, res, phases, partial):
    """Rewrite `out` atomically with what has been measured so far.

    A killed run therefore leaves valid JSON naming the finished phases.
    """
    doc = dict(res)
    doc["partial"] = partial
    doc["completed_phases"] = list(phases)
    tmp = f"{out}.tmp"
    Path(tmp).write_text(json.dumps(doc, indent=2))
    os.replace(tmp, out)


def main():
    global COMMAND_TIMEOUT
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--velody", default=f"target/release/velody{EXE}")
    ap.add_argument("--files", type=int)
    ap.add_argument("--snapshots", type=int)
    ap.add_argument("--change", type=int, default=1, help="files changed per snapshot")
    ap.add_argument("--workdir")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--preset", choices=PRESETS)
    ap.add_argument("--keep", action="store_true", help="keep the work directory")
    ap.add_argument("--objects", choices=["files", "database"], default="files",
                    help="object backend passed to velody init")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--defender-note", default=None,
                    help="Windows: free text, e.g. 'workdir excluded from real-time scanning'")
    ap.add_argument("--command-timeout", type=float, default=None, metavar="SECONDS",
                    help="kill any single velody command after this long (default: no limit)")
    a = ap.parse_args()
    COMMAND_TIMEOUT = a.command_timeout

    files, snaps = PRESETS[a.preset or "small"]
    files = a.files or files
    snaps = a.snapshots or snaps
    if snaps < 2 or a.change > files:
        sys.exit("need --snapshots >= 2 and --change <= --files")

    velody = Path(a.velody).resolve()
    if not velody.exists():
        sys.exit(f"velody binary not found: {velody}")
    repo_root = Path(__file__).resolve().parents[2]
    if a.workdir:
        base = Path(a.workdir).resolve()
        base.mkdir(parents=True, exist_ok=True)
    else:
        base = Path(tempfile.mkdtemp(prefix="velody-scale-")).resolve()
    if repo_root == base or repo_root in base.parents:
        sys.exit("workdir must not be inside the repository checkout")
    work = base / "repo"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir()

    rng = random.Random(a.seed)

    def log(m):
        print(m, file=sys.stderr, flush=True)

    log(f"generating {files} files")
    paths = [rel(i) for i in range(files)]
    for p in paths:
        f = work / p
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_bytes(content(rng, p, 0))

    run(velody, ["init", "--objects", a.objects], work)
    res = {"machine": machine_info(a.defender_note)}
    phases = []

    def phase(name):
        phases.append(name)
        write_partial(a.out, res, phases, True)

    saves, hashes = [], []
    for s in range(snaps):
        if s:
            for p in rng.sample(paths, a.change):
                (work / p).write_bytes(content(rng, p, s))
        dt, out = run(velody, ["save", f"snapshot {s}"], work)
        m = re.search(r"Saved ([0-9a-f]{8,64})", out)
        if not m:
            sys.exit(f"cannot parse save output: {out!r}")
        saves.append(dt)
        hashes.append(m.group(1))
        if s % max(1, snaps // 20) == 0:
            log(f"save {s + 1}/{snaps}: {dt:.3f}s")
            res["save_all_s"] = saves
            phase("first_save" if s == 0 else f"saves_{s + 1}")

    k = max(1, snaps // 10)
    res.update({
        "save_first_10pct": summarize(saves[:k]),
        "save_last_10pct": summarize(saves[-k:]),
        "save_first_s": saves[0],
        "save_all_s": saves,
    })
    res["save_history0_median_s"], res["save_ratio_vs_history0"] = history0_ratio(saves)
    res["save_growth_median_ratio"] = (
        res["save_last_10pct"]["median_s"] / res["save_first_10pct"]["median_s"])
    phase("saves")

    velody_dir = work / ".velo"
    db = velody_dir / "velo.db"
    res["velody_dir_bytes"] = dir_size(velody_dir)
    res["db_bytes"] = db.stat().st_size
    con = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)
    res["file_map_rows"] = con.execute("SELECT COUNT(*) FROM file_map").fetchone()[0]
    con.close()
    phase("sizes")

    hist_file = paths[files // 2]
    mid = hashes[snaps // 2 - 1]
    res["status_clean_s"] = median_of(3, velody, ["status"], work)
    phase("status")
    res["history_full_s"] = median_of(3, velody, ["history", "--oneline", "--limit", str(snaps)], work)
    res["history_file_s"] = median_of(3, velody, ["history", "--file", hist_file, "--oneline"], work)
    phase("history")
    ts = []
    for _ in range(3):
        t1 = run(velody, ["restore", mid, "--force"], work)[0]
        t2 = run(velody, ["restore", hashes[-1], "--force"], work)[0]
        ts.append((t1, t2))
    res["restore_mid_s"] = statistics.median(t[0] for t in ts)
    res["restore_tip_s"] = statistics.median(t[1] for t in ts)
    phase("restore")
    res["gc_s"] = median_of(3, velody, ["gc"], work)
    phase("gc")
    res["fsck_s"] = median_of(3, velody, ["fsck"], work)
    phase("fsck")
    res["open_s"] = median_of(5, velody, ["branches"], work)
    res["velody_dir_bytes_after_gc"] = dir_size(velody_dir)

    res["env"] = {
        "platform": platform.platform(),
        "cpu": cpu_name(),
        "velody_version": run(velody, ["--version"], work)[1].strip(),
        "workdir": str(work),
        "files": files, "snapshots": snaps, "change_per_snapshot": a.change,
        "objects": a.objects,
        "seed": a.seed, "python": platform.python_version(),
    }
    if os.name == "nt":
        res["env"]["defender_note"] = a.defender_note or "not provided: assume real-time scanning is ON"

    write_partial(a.out, res, phases + ["done"], False)

    def mb(b):
        return f"{b / 1048576:.1f} MiB"

    def ms(s):
        return f"{s * 1000:.0f} ms"

    f, l = res["save_first_10pct"], res["save_last_10pct"]
    rows = [
        ("files x snapshots (change/snap)", f"{files} x {snaps} ({a.change})"),
        ("save, first 10% median / p95", f"{ms(f['median_s'])} / {ms(f['p95_s'])}"),
        ("save, last 10% median / p95", f"{ms(l['median_s'])} / {ms(l['p95_s'])}"),
        ("save growth (last/first median)", f"{res['save_growth_median_ratio']:.2f}x"),
        ("one-file save, history-0 median", "n/a" if res["save_ratio_vs_history0"] is None
         else f"{ms(res['save_history0_median_s'])} (tail {res['save_ratio_vs_history0']:.2f}x)"),
        (".velo size", mb(res["velody_dir_bytes"])),
        ("velo.db size", mb(res["db_bytes"])),
        ("file_map rows", str(res["file_map_rows"])),
        ("status (clean)", ms(res["status_clean_s"])),
        ("history (full)", ms(res["history_full_s"])),
        ("history --file", ms(res["history_file_s"])),
        ("restore snapshot M/2", ms(res["restore_mid_s"])),
        ("restore back to tip", ms(res["restore_tip_s"])),
        ("gc", ms(res["gc_s"])),
        ("fsck", ms(res["fsck_s"])),
        ("open (branches)", ms(res["open_s"])),
        ("platform", res["env"]["platform"]),
        ("velody", res["env"]["velody_version"]),
    ]
    if "defender_note" in res["env"]:
        rows.append(("Defender", res["env"]["defender_note"]))
    print("| metric | value |\n|---|---|")
    for name, val in rows:
        print(f"| {name} | {val} |")

    if not a.keep:
        shutil.rmtree(work if a.workdir else base, ignore_errors=True)


if __name__ == "__main__":
    main()
