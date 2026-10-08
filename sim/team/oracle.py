#!/usr/bin/env python
"""Deterministic checks for the velo team simulation. Never mutates a clone.

    python sim/team/oracle.py <SIMDIR> <label> [--converge]

Checks
  fsck          `velo fsck` exits 0 on the remote and on every clone
  tests         a fresh clone of the remote passes `python -m unittest discover -s tests`
  ledger        every token an agent logged as merged to main (and never later
                logged as removed) is present in main on the remote   -> data loss
  attribution   blame author of a token's line equals the agent that wrote it (warning only:
                rebase / squash / amend may legitimately change it)
  converge      (--converge) every clone's local `main` equals the remote's `main`,
                no merge/rebase left in progress, and clones are clean

Ledger: SIMDIR/ledger/<agent>.jsonl, one JSON object per line:
  {"agent","round","token","status":"merged_main"|"branch_only"|"removed","branch","file"}
Prints one JSON object; exit code 0 always (the workflow reads `ok`).
"""
import json, os, re, shutil, subprocess, sys
from pathlib import Path

sim = Path(sys.argv[1]).resolve()
label = sys.argv[2] if len(sys.argv) > 2 else "check"
converge = "--converge" in sys.argv
EXE = ".exe" if os.name == "nt" else ""
real = sim / "bin" / "real" / f"velo{EXE}"
env = dict(os.environ, VELO_AUTHOR_NAME="oracle", VELO_AUTHOR_EMAIL="oracle@team.test")


def velo(cwd, *a, timeout=120):
    try:
        r = subprocess.run([str(real), *a], cwd=cwd, env=env, capture_output=True, text=True,
                           timeout=timeout, encoding="utf-8", errors="replace")
        return r.returncode, (r.stdout + r.stderr)
    except subprocess.TimeoutExpired:
        return 124, "TIMEOUT"


def branch_heads(cwd):
    rc, out = velo(cwd, "branches")
    heads = {}
    for line in out.splitlines():
        m = re.match(r"^\s*\*?\s*(\S+)\s+([0-9a-f]{16,})\b", line)
        if m:
            heads[m.group(1)] = m.group(2)
    return heads


import time
with open(sim / "rounds.jsonl", "a") as _f:
    _f.write(json.dumps({"label": label, "ts": time.time()}) + "\n")

problems, warnings = [], []
remote = sim / "remote"
clones = sorted(p for p in (sim / "clones").iterdir() if p.is_dir())

# fsck ---------------------------------------------------------------------
fsck, fsck_ms = {}, {}
for name, path in [("remote", remote)] + [(c.name, c) for c in clones]:
    _t = time.perf_counter()
    rc, out = velo(path, "fsck")
    fsck[name] = rc
    fsck_ms[name] = round((time.perf_counter() - _t) * 1000)
    if rc != 0:
        problems.append(f"fsck failed in {name}: {out.strip()[-600:]}")

remote_heads = branch_heads(remote)

# fresh clone + project tests ------------------------------------------------
verify = sim / f"verify-{label}"
if verify.exists():
    shutil.rmtree(verify, ignore_errors=True)
rc, out = velo(sim, "clone", str(remote), str(verify))
tests = {"ran": False}
if rc != 0:
    problems.append(f"fresh clone of remote failed: {out.strip()[-400:]}")
else:
    # a clone checks out the default branch; make sure it is main
    velo(verify, "switch", "main", "--force")
    tr = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=verify,
                        capture_output=True, text=True, encoding="utf-8", errors="replace")
    tests = {"ran": True, "rc": tr.returncode, "tail": (tr.stdout + tr.stderr).strip()[-500:]}
    if tr.returncode != 0:
        problems.append(f"project tests fail on remote main: {tests['tail']}")
    # conflict markers committed to main?
    for f in verify.rglob("*"):
        if f.is_file() and ".velo" not in f.parts and f.suffix in {".py", ".md", ".json", ".txt"}:
            try:
                t = f.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            if re.search(r"^(<{7}|>{7}) ", t, re.M):
                problems.append(f"conflict markers committed in {f.relative_to(verify)}")
            if f.suffix == ".json":
                try:
                    json.loads(t)
                except ValueError as e:
                    problems.append(f"{f.relative_to(verify)} is not valid JSON on main: {e}")

# ledger ---------------------------------------------------------------------
claims, removed = {}, set()
ledger_dir = sim / "ledger"
if ledger_dir.exists():
    for lf in sorted(ledger_dir.glob("*.jsonl")):
        for ln in lf.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                e = json.loads(ln)
            except ValueError:
                warnings.append(f"unparseable ledger line in {lf.name}: {ln[:80]}")
                continue
            if e.get("status") == "removed":
                removed.add(e.get("token"))
            elif e.get("status") == "merged_main":
                claims[e.get("token")] = e
lost, misattributed = [], []
if verify.exists():
    corpus = {}
    for f in verify.rglob("*"):
        if f.is_file() and ".velo" not in f.parts:
            try:
                corpus[f] = f.read_text(encoding="utf-8", errors="replace")
            except OSError:
                pass
    for tok, e in claims.items():
        if tok in removed or not tok:
            continue
        hit = next((f for f, t in corpus.items() if f"[[{tok}]]" in t), None)
        if hit is None:
            lost.append(tok)
            continue
        rc, out = velo(verify, "blame", str(hit.relative_to(verify)).replace("\\", "/"))
        for line in out.splitlines():
            if f"[[{tok}]]" in line:
                parts = line.split()
                if len(parts) > 3 and parts[3] != e.get("agent"):
                    misattributed.append(f"{tok}: blame says {parts[3]}, ledger says {e.get('agent')}")
                break
    if lost:
        problems.append(f"{len(lost)} merged work item(s) missing from remote main (data loss?): {lost[:15]}")
    if misattributed:
        warnings.append(f"{len(misattributed)} blame attribution mismatch(es): {misattributed[:10]}")

# convergence ----------------------------------------------------------------
clone_state = {}
for c in clones:
    velo(c, "fetch", timeout=120)
    heads = branch_heads(c)
    rc, st = velo(c, "status")
    clone_state[c.name] = {"main": heads.get("main"), "dirty": "clean" not in st.lower(),
                           "status": st.strip()[:400]}
    if converge:
        if heads.get("main") != remote_heads.get("main"):
            problems.append(f"{c.name}: local main {str(heads.get('main'))[:10]} != remote main "
                            f"{str(remote_heads.get('main'))[:10]}")
        low = st.lower()
        if "merge in progress" in low or "rebase in progress" in low or "conflict" in low:
            problems.append(f"{c.name}: merge/rebase/conflict state left behind: {st.strip()[:200]}")

print(json.dumps({
    "label": label, "ok": not problems, "problems": problems, "warnings": warnings,
    "fsck": fsck, "fsckMs": fsck_ms, "tests": tests, "remoteBranches": remote_heads,
    "ledgerClaims": len(claims), "clones": clone_state,
}, indent=2))
