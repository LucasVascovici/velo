#!/usr/bin/env python3
"""Scale benchmark for velo: many files, long histories.

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
}
FILES_PER_DIR = 100
FILE_BYTES = 200
EXE = ".exe" if os.name == "nt" else ""


def run(velo, args, cwd, check=True):
    """Run one velo command; return (seconds, stdout)."""
    t0 = time.perf_counter()
    p = subprocess.run([str(velo), *args], cwd=cwd, capture_output=True,
                       text=True, encoding="utf-8", errors="replace")
    dt = time.perf_counter() - t0
    if check and p.returncode != 0:
        sys.exit(f"velo {' '.join(args)} failed ({p.returncode}):\n{p.stdout}\n{p.stderr}")
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


def median_of(n, velo, args, cwd):
    return statistics.median(run(velo, args, cwd)[0] for _ in range(n))


def cpu_name():
    if platform.system() == "Linux":
        try:
            for line in open("/proc/cpuinfo"):
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
        except OSError:
            pass
    return platform.processor() or "unknown"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--velo", default=f"target/release/velo{EXE}")
    ap.add_argument("--files", type=int)
    ap.add_argument("--snapshots", type=int)
    ap.add_argument("--change", type=int, default=1, help="files changed per snapshot")
    ap.add_argument("--workdir")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--preset", choices=PRESETS)
    ap.add_argument("--keep", action="store_true", help="keep the work directory")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--defender-note", default=None,
                    help="Windows: free text, e.g. 'workdir excluded from real-time scanning'")
    a = ap.parse_args()

    files, snaps = PRESETS[a.preset or "small"]
    files = a.files or files
    snaps = a.snapshots or snaps
    if snaps < 2 or a.change > files:
        sys.exit("need --snapshots >= 2 and --change <= --files")

    velo = Path(a.velo).resolve()
    if not velo.exists():
        sys.exit(f"velo binary not found: {velo}")
    repo_root = Path(__file__).resolve().parents[2]
    if a.workdir:
        base = Path(a.workdir).resolve()
        base.mkdir(parents=True, exist_ok=True)
    else:
        base = Path(tempfile.mkdtemp(prefix="velo-scale-")).resolve()
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

    run(velo, ["init"], work)
    saves, hashes = [], []
    for s in range(snaps):
        if s:
            for p in rng.sample(paths, a.change):
                (work / p).write_bytes(content(rng, p, s))
        dt, out = run(velo, ["save", f"snapshot {s}"], work)
        m = re.search(r"Saved ([0-9a-f]{8,64})", out)
        if not m:
            sys.exit(f"cannot parse save output: {out!r}")
        saves.append(dt)
        hashes.append(m.group(1))
        if s % max(1, snaps // 20) == 0:
            log(f"save {s + 1}/{snaps}: {dt:.3f}s")

    k = max(1, snaps // 10)
    res = {
        "save_first_10pct": summarize(saves[:k]),
        "save_last_10pct": summarize(saves[-k:]),
        "save_first_s": saves[0],
        "save_all_s": saves,
    }
    res["save_growth_median_ratio"] = (
        res["save_last_10pct"]["median_s"] / res["save_first_10pct"]["median_s"])

    velo_dir = work / ".velo"
    db = velo_dir / "velo.db"
    res["velo_dir_bytes"] = dir_size(velo_dir)
    res["db_bytes"] = db.stat().st_size
    con = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)
    res["file_map_rows"] = con.execute("SELECT COUNT(*) FROM file_map").fetchone()[0]
    con.close()

    hist_file = paths[files // 2]
    mid = hashes[snaps // 2 - 1]
    res["status_clean_s"] = median_of(3, velo, ["status"], work)
    res["history_full_s"] = median_of(3, velo, ["history", "--oneline", "--limit", str(snaps)], work)
    res["history_file_s"] = median_of(3, velo, ["history", "--file", hist_file, "--oneline"], work)
    ts = []
    for _ in range(3):
        t1 = run(velo, ["restore", mid, "--force"], work)[0]
        t2 = run(velo, ["restore", hashes[-1], "--force"], work)[0]
        ts.append((t1, t2))
    res["restore_mid_s"] = statistics.median(t[0] for t in ts)
    res["restore_tip_s"] = statistics.median(t[1] for t in ts)
    res["gc_s"] = median_of(3, velo, ["gc"], work)
    res["fsck_s"] = median_of(3, velo, ["fsck"], work)
    res["open_s"] = median_of(5, velo, ["branches"], work)
    res["velo_dir_bytes_after_gc"] = dir_size(velo_dir)

    res["env"] = {
        "platform": platform.platform(),
        "cpu": cpu_name(),
        "velo_version": run(velo, ["--version"], work)[1].strip(),
        "workdir": str(work),
        "files": files, "snapshots": snaps, "change_per_snapshot": a.change,
        "seed": a.seed, "python": platform.python_version(),
    }
    if os.name == "nt":
        res["env"]["defender_note"] = a.defender_note or "not provided: assume real-time scanning is ON"

    Path(a.out).write_text(json.dumps(res, indent=2))

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
        (".velo size", mb(res["velo_dir_bytes"])),
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
        ("velo", res["env"]["velo_version"]),
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
