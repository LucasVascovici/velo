#!/usr/bin/env python
"""Turn the command log of a simulation run into material for the performance and UX reviews.

    python sim/team/digest.py <SIMDIR>

Reads   <SIMDIR>/cmdlog/<agent>.jsonl   (written by the per-persona wrapper: every velo call)
        <SIMDIR>/rounds.jsonl           (oracle timestamps, used to label rounds)
Writes  <SIMDIR>/perf/cmdstats.json     per-subcommand count / failures / p50 / p95 / max ms,
                                        slowest calls, lock-contention hits
        <SIMDIR>/ux/<agent>.md          per-developer digest: stats, every failure with what they did
                                        next, help lookups, a sample of successes (size-capped)
        <SIMDIR>/ux/clusters.json       failures grouped by error text across developers
Prints a one-line summary.
"""
import json, re, statistics, sys
from collections import Counter, defaultdict
from pathlib import Path

sim = Path(sys.argv[1]).resolve()
(sim / "perf").mkdir(exist_ok=True)
(sim / "ux").mkdir(exist_ok=True)
MD_CAP = 70_000

recs = []
for f in sorted((sim / "cmdlog").glob("*.jsonl")):
    if f.stem == "oracle":
        continue
    for ln in f.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            recs.append(json.loads(ln))
        except ValueError:
            pass
recs.sort(key=lambda r: r["ts"])

bounds = []
rf = sim / "rounds.jsonl"
if rf.exists():
    for ln in rf.read_text().splitlines():
        try:
            bounds.append(json.loads(ln))
        except ValueError:
            pass
bounds.sort(key=lambda b: b["ts"])


def round_of(ts):
    for b in bounds:
        if ts <= b["ts"]:
            return b["label"].replace("round", "r")
    return "after-last-check"


def sub(r):
    a = [x for x in r["args"] if not x.startswith("-")]
    if not a:
        return "(" + (r["args"][0] if r["args"] else "no-args") + ")"
    if a[0] in ("stash", "remote", "bundle") and len(a) > 1:
        return f"{a[0]} {a[1]}"
    return a[0]


def cmdline(r):
    return "velo " + " ".join(x if re.fullmatch(r"[\w./:@=,+-]+", x) else json.dumps(x) for x in r["args"])


def first_line(out):
    for ln in out.splitlines():
        ln = ln.strip()
        if ln:
            return ln
    return ""


def norm(line):
    line = re.sub(r"\b[0-9a-f]{8,64}\b", "<hash>", line)
    line = re.sub(r"[A-Za-z]:[/\\][^\s'\"]*|/[\w./-]{6,}", "<path>", line)
    line = re.sub(r"'[^']{1,60}'", "'<x>'", line)
    return re.sub(r"\d+", "N", line)[:160]


is_help = lambda r: bool(r["args"]) and (r["args"][0] == "help" or "--help" in r["args"] or "-h" in r["args"])

# ── perf stats ──────────────────────────────────────────────────────────────
by_sub = defaultdict(list)
for r in recs:
    by_sub[sub(r)].append(r)


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))]


stats = {}
for k, rs in sorted(by_sub.items(), key=lambda kv: -len(kv[1])):
    ms = [r["ms"] for r in rs]
    stats[k] = {"count": len(rs), "failures": sum(1 for r in rs if r["rc"] != 0),
                "p50_ms": round(statistics.median(ms), 1), "p95_ms": round(pct(ms, 0.95), 1), "max_ms": round(max(ms), 1)}
slowest = sorted(recs, key=lambda r: -r["ms"])[:20]
lockre = re.compile(r"locked|busy|timed? ?out|another (process|velo)|lock file|could not acquire|killed after", re.I)
contention = [{"agent": r["agent"], "cmd": cmdline(r), "ms": r["ms"], "rc": r["rc"], "out": first_line(r["out"])[:200]}
              for r in recs if lockre.search(r["out"])]
(sim / "perf" / "cmdstats.json").write_text(json.dumps({
    "note": "measured while all developers ran concurrently (CPU/disk contention): compare ratios, not absolutes. "
            "The idle-machine numbers are in results.json.",
    "totalCalls": len(recs), "perSubcommand": stats,
    "slowest20": [{"agent": r["agent"], "cmd": cmdline(r), "ms": r["ms"], "rc": r["rc"], "round": round_of(r["ts"]),
                   "cwd": r["cwd"].rsplit("/", 1)[-1]} for r in slowest],
    "contentionHits": contention[:60], "contentionCount": len(contention),
}, indent=2))

# ── UX digests ──────────────────────────────────────────────────────────────
by_agent = defaultdict(list)
for r in recs:
    by_agent[r["agent"]].append(r)

clusters = defaultdict(lambda: {"count": 0, "agents": set(), "examples": []})
for name, rs in by_agent.items():
    fails = [i for i, r in enumerate(rs) if r["rc"] != 0]
    helps = [r for r in rs if is_help(r)]
    out = [f"# UX digest: {name}", "",
           f"- commands run: {len(rs)}; non-zero exit: {len(fails)} ({100*len(fails)//max(1,len(rs))}%); help lookups: {len(helps)}",
           f"- distinct subcommands used: {len({sub(r) for r in rs})}",
           "- subcommand mix: " + ", ".join(f"{k}x{c}" for k, c in Counter(sub(r) for r in rs).most_common(15)), ""]
    # failures, grouped so repeated identical errors do not eat the budget
    out += ["## Failures (non-zero exit), each with the developer's next steps", ""]
    seen = Counter()
    for i in fails:
        r = rs[i]
        key = (sub(r), norm(first_line(r["out"])))
        seen[key] += 1
        c = clusters[key]
        c["count"] += 1
        c["agents"].add(name)
        if len(c["examples"]) < 3:
            c["examples"].append(f"{name}: {cmdline(r)} -> {first_line(r['out'])[:160]}")
        if seen[key] > 2:
            continue
        nxt = rs[i + 1:i + 4]
        recovered = next((x for x in rs[i + 1:i + 8] if sub(x) == sub(r) and x["rc"] == 0), None)
        block = [f"### [{round_of(r['ts'])}] #{i}  `{cmdline(r)}`  exit {r['rc']}  ({r['ms']:.0f} ms)  cwd={r['cwd'].rsplit('/', 1)[-1]}",
                 "```", r["out"].strip()[:700], "```",
                 "next: " + "; ".join(f"`{cmdline(x)}`→{x['rc']}" for x in nxt) if nxt else "next: (end)",
                 ("recovered by: `" + cmdline(recovered) + "`") if recovered else "recovered by: (same command did not succeed within 7 calls)", ""]
        out += block
    rep = {k: c for k, c in seen.items() if c > 2}
    if rep:
        out += ["(identical failures repeated more times than shown: " + "; ".join(f"{k[0]} '{k[1]}' x{c}" for k, c in rep.items()) + ")", ""]
    out += ["## Help lookups", ""]
    out += [f"- [{round_of(r['ts'])}] `{cmdline(r)}`" for r in helps[:60]] or ["(none)"]
    out += ["", "## Sample of successful calls (every few, trimmed)", ""]
    ok = [r for r in rs if r["rc"] == 0 and not is_help(r)]
    step = max(1, len(ok) // 40)
    for r in ok[::step][:40]:
        out.append(f"- [{round_of(r['ts'])}] `{cmdline(r)}` → {first_line(r['out'])[:140]}")
    text = "\n".join(out)
    if len(text) > MD_CAP:
        text = text[:MD_CAP] + "\n\n[digest truncated at size cap; later failures omitted]"
    (sim / "ux" / f"{name}.md").write_text(text, encoding="utf-8")

(sim / "ux" / "clusters.json").write_text(json.dumps(sorted(
    [{"subcommand": k[0], "errorLine": k[1], "count": c["count"], "agents": sorted(c["agents"]), "examples": c["examples"]}
     for k, c in clusters.items()], key=lambda x: -x["count"])[:60], indent=2))
print(f"{len(recs)} calls from {len(by_agent)} developers; {sum(1 for r in recs if r['rc']!=0)} non-zero exits; "
      f"{len(contention)} contention-looking outputs; digests in {sim/'ux'}")
