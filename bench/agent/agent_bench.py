#!/usr/bin/env python3
"""Agent-checkpointing benchmark: velo against the alternatives an agent author
would actually reach for. See bench/agent/README.md for the method.

Systems: velo, shadow git, git worktree per attempt, directory copy per step,
append-only JSONL log, SQLite blobs. Workloads: N steps editing 1..20 files in
a F-file project; a branch-per-attempt tree with 8 attempts and a merged
winner; restore to step n 100 times; "who wrote this line" across the run.
"""
import argparse
import datetime
import json
import os
import platform
import random
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ATTEMPTS = 8
ATTEMPT_STEPS = 10
WHO_QUERIES = 20
LINES_PER_FILE = 20
RESTORES = 100
MERGE_STEP = 10**6
WORDS = ("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu "
         "nu xi omicron pi rho sigma tau upsilon phi chi psi omega").split()

PROVENANCE = {
    "velo (5.2 API)": "as velo",
    "velo": "ids recompute from content; author and metadata are hashed into "
            "each snapshot id; signatures can be attached",
    "shadow git": "object hashes verify content; author and message per commit "
                  "only by discipline, nothing binds them to a run",
    "git worktree": "as shadow git",
    "dir copy": "none",
    "jsonl log": "none",
    "sqlite blobs": "none",
}


# ---------------------------------------------------------------- workload

def rand_line(rng, i):
    return f"L{i} " + " ".join(rng.choice(WORDS) for _ in range(8)) + f" {rng.randrange(10**6)}"


def make_project(rng, nfiles):
    files = {}
    for i in range(nfiles):
        path = f"src/d{i % 20:02d}/f{i:04d}.txt"
        files[path] = "\n".join(rand_line(rng, n) for n in range(LINES_PER_FILE)) + "\n"
    return files


def make_edit(rng, paths, content, tag):
    """Pick 1..20 files and replace 1..3 lines in each. The line count never
    changes, so a line number names the same line across the whole history.
    Returns {path: (new_text, [edited line numbers])}."""
    changed = {}
    for p in rng.sample(paths, rng.randint(1, 20)):
        lines = content[p].split("\n")
        edited = set()
        for _ in range(rng.randint(1, 3)):
            n = rng.randrange(LINES_PER_FILE)
            lines[n] = f"L{n} edit-{tag} " + " ".join(rng.choice(WORDS) for _ in range(5))
            edited.add(n)
        changed[p] = ("\n".join(lines), sorted(edited))
    return changed


def dir_bytes(path):
    total = 0
    for root, _dirs, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return total


def write_files(base, files):
    for p, text in files.items():
        fp = Path(base) / p
        fp.parent.mkdir(parents=True, exist_ok=True)
        fp.write_bytes(text.encode())


def run(cmd, cwd=None, env=None):
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{cmd} failed: {r.stderr.strip()}")
    return r.stdout


def scan_back(versions, line):
    """versions yields (step, text) newest first. Returns the step that
    introduced the line's final content."""
    final = who = None
    for step, text in versions:
        cur = text.split("\n")[line]
        if final is None:
            final, who = cur, step
        elif cur == final:
            who = step
        else:
            break
    return who


# ---------------------------------------------------------------- adapters
# Common interface:
#   setup(project)             create the store
#   work_dir                   directory the driver writes edits into
#   checkpoint(step, changed)  changed: {path: text}; files already on disk
#   branch(base_step, name)    start an attempt from base_step
#   merge(winner)              merge the winner attempt into main
#   restore(step, dest)        materialise step into the empty directory dest
#   who_wrote(path, line)      step that last wrote the line on main
#   disk()                     bytes on disk of the store


class Velo52:
    """The 5.2 calling convention, kept unchanged as the baseline row."""
    name = "velo (5.2 API)"

    def __init__(self, root):
        import velo
        self.velo = velo
        self.root = Path(root)
        self.work_dir = self.root / "work"   # unused by velo: bytes go in directly
        self.work_dir.mkdir(parents=True)
        self.repo_dir = self.root / "repo"
        self.snap = {}        # step -> snapshot id
        self.objs = {}        # path -> object id in the current attempt's tree
        self.branch_name = "main"
        self.tip = self.main_tip = None

    def _refresh(self, sid):
        self.objs = {t.path: t.object for t in self.repo.tree_at(sid)}

    def setup(self, project):
        self.repo = self.velo.Repo.init(str(self.repo_dir))

    def checkpoint(self, step, changed):
        velo = self.velo
        # Untouched files are carried forward by object id, without their bytes.
        entries = [velo.TreeEntry.stored(p, o) for p, o in self.objs.items() if p not in changed]
        entries += [velo.TreeEntry.file(p, t.encode()) for p, t in changed.items()]
        sid = self.repo.save_tree(
            branch=self.branch_name, message=f"step {step}", entries=entries,
            parent=self.tip, author=self.velo.Author("agent-run-1"),
            meta={"agent": {"run": "run-1", "tool_call_id": str(step), "model": "bench"}})
        self.snap[step] = self.tip = sid
        if self.branch_name == "main":
            self.main_tip = sid
        self._refresh(sid)

    def branch(self, base_step, name):
        self.repo.create_branch(name, at=self.snap[base_step])
        self.branch_name = name
        self.tip = self.snap[base_step]
        self._refresh(self.tip)

    def merge(self, winner):
        merged = self.repo.merge_commit(branch="main", ours=self.main_tip,
                                        theirs=self.repo.branch_tip(winner),
                                        message=f"take {winner}")
        self.branch_name = "main"
        self.tip = self.main_tip = merged

    def restore(self, step, dest):
        # tree_at lists the files, read_file_at fetches each one's bytes.
        sid = self.snap[step]
        for t in self.repo.tree_at(sid):
            fp = Path(dest) / t.path
            fp.parent.mkdir(parents=True, exist_ok=True)
            fp.write_bytes(self.repo.read_file_at(sid, t.path))

    def who_wrote(self, path, line):
        origin = self.repo.blame(path, at=self.main_tip).lines[line].origin
        return int(origin.message.split()[1])

    def disk(self):
        return dir_bytes(self.repo_dir)


class Velo(Velo52):
    """The 5.3 calling convention: save_changes sends only the changed files and
    the core carries the parent's tree forward, so the harness neither re-reads
    the tree nor keeps an entry list; restore is one materialize call."""
    name = "velo"

    def checkpoint(self, step, changed):
        saved = self.repo.save_changes(
            branch=self.branch_name, message=f"step {step}", changes=dict(changed),
            parent=self.tip, author=self.velo.Author("agent-run-1"),
            meta={"agent": {"run": "run-1", "tool_call_id": str(step), "model": "bench"}})
        self.snap[step] = self.tip = saved.snapshot
        if self.branch_name == "main":
            self.main_tip = saved.snapshot

    def branch(self, base_step, name):
        self.repo.create_branch(name, at=self.snap[base_step])
        self.branch_name = name
        self.tip = self.snap[base_step]

    def restore(self, step, dest):
        self.repo.materialize(self.snap[step], dest)


class ShadowGit:
    name = "shadow git"

    def __init__(self, root):
        self.root = Path(root)
        self.work_dir = self.main_work = self.root / "work"
        self.work_dir.mkdir(parents=True)
        self.git_dir = self.root / "shadow.git"
        self.commit = {}
        self.env = dict(os.environ, GIT_AUTHOR_NAME="agent-run-1", GIT_AUTHOR_EMAIL="a@bench",
                        GIT_COMMITTER_NAME="agent-run-1", GIT_COMMITTER_EMAIL="a@bench")

    def git(self, *args, cwd=None, env=None):
        cwd = cwd or self.work_dir
        return run(["git", f"--git-dir={self.git_dir}", f"--work-tree={cwd}",
                    "-c", "core.autocrlf=false", *args], cwd=cwd, env=env or self.env)

    def setup(self, project):
        run(["git", "init", "-q", "--bare", "-b", "main", str(self.git_dir)])
        run(["git", f"--git-dir={self.git_dir}", "config", "core.bare", "false"])

    def checkpoint(self, step, changed):
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", f"step {step}")
        self.commit[step] = self.git("rev-parse", "HEAD").strip()

    def branch(self, base_step, name):
        self.git("checkout", "-q", "-f", "-B", name, self.commit[base_step])

    def merge(self, winner):
        self.git("checkout", "-q", "-f", "main")
        self.git("merge", "-q", "--no-ff", "-m", f"take {winner}", winner)

    def restore(self, step, dest):
        # `git checkout <commit> -- .` into a separate work tree with its own
        # index, so the shadow work tree and index are untouched.
        env = dict(self.env, GIT_INDEX_FILE=str(self.root / "restore.index"))
        run(["git", f"--git-dir={self.git_dir}", f"--work-tree={dest}", "checkout", "-q", "-f",
             self.commit[step], "--", "."], cwd=dest, env=env)

    def who_wrote(self, path, line):
        out = self.git("blame", "-L", f"{line + 1},{line + 1}", "--porcelain", "main", "--",
                       path, cwd=self.main_work)
        msg = self.git("log", "-1", "--format=%s", out.split()[0], cwd=self.main_work)
        return int(msg.split()[1])

    def disk(self):
        return dir_bytes(self.git_dir)


class GitWorktree(ShadowGit):
    """An ordinary git repository; each attempt gets its own `git worktree`."""
    name = "git worktree"

    def __init__(self, root):
        super().__init__(root)
        self.git_dir = self.main_work / ".git"
        self.wt_root = self.root / "wt"

    def git(self, *args, cwd=None, env=None):
        # Inside a worktree directory, its .git file selects the right checkout.
        return run(["git", "-c", "core.autocrlf=false", *args], cwd=cwd or self.work_dir,
                   env=env or self.env)

    def setup(self, project):
        run(["git", "init", "-q", "-b", "main", str(self.main_work)])

    def branch(self, base_step, name):
        wt = self.wt_root / name
        self.wt_root.mkdir(exist_ok=True)
        self.git("worktree", "add", "-q", "-b", name, str(wt), self.commit[base_step],
                 cwd=self.main_work)
        self.work_dir = wt

    def merge(self, winner):
        self.work_dir = self.main_work
        self.git("merge", "-q", "--no-ff", "-m", f"take {winner}", winner)

    def disk(self):
        # The object store plus every attempt's checked-out tree: that is the
        # price of a worktree per attempt.
        return dir_bytes(self.git_dir) + (dir_bytes(self.wt_root) if self.wt_root.exists() else 0)


class DirCopy:
    name = "dir copy"

    def __init__(self, root):
        self.root = Path(root)
        self.work_dir = self.main_work = self.root / "work"
        self.work_dir.mkdir(parents=True)
        self.steps = self.root / "steps"
        self.attempts = {}
        self.last_main = 0

    def setup(self, project):
        self.steps.mkdir()

    def checkpoint(self, step, changed):
        shutil.copytree(self.work_dir, self.steps / str(step))
        if self.work_dir == self.main_work:
            self.last_main = step

    def branch(self, base_step, name):
        wd = self.root / f"attempt-{name}"
        shutil.copytree(self.steps / str(base_step), wd)
        self.attempts[name] = wd
        self.work_dir = wd

    def merge(self, winner):
        shutil.rmtree(self.main_work)
        shutil.copytree(self.attempts[winner], self.main_work)
        self.work_dir = self.main_work
        self.checkpoint(MERGE_STEP, {})

    def restore(self, step, dest):
        shutil.copytree(self.steps / str(step), dest, dirs_exist_ok=True)

    def who_wrote(self, path, line):
        versions = ((s, (self.steps / str(s) / path).read_text())
                    for s in range(self.last_main, -1, -1))
        return scan_back(versions, line)

    def disk(self):
        return dir_bytes(self.steps) + sum(dir_bytes(d) for d in self.attempts.values())


class JsonlLog:
    name = "jsonl log"

    def __init__(self, root):
        self.root = Path(root)
        self.work_dir = self.root / "work"
        self.work_dir.mkdir(parents=True)
        self.log = self.root / "log.jsonl"
        self.cur = "main"
        self.attempt_files = {}

    def setup(self, project):
        self.fh = open(self.log, "a", encoding="utf-8")

    def _append(self, rec):
        self.fh.write(json.dumps(rec) + "\n")
        self.fh.flush()
        os.fsync(self.fh.fileno())

    def checkpoint(self, step, changed):
        self._append({"step": step, "branch": self.cur, "files": changed})
        if self.cur != "main":
            self.attempt_files[self.cur].update(changed)

    def branch(self, base_step, name):
        self.cur = name
        self.attempt_files[name] = {}

    def merge(self, winner):
        self.cur = "main"
        self._append({"step": MERGE_STEP, "branch": "main", "merge": winner,
                      "files": self.attempt_files[winner]})

    def _main_records(self):
        with open(self.log, encoding="utf-8") as f:
            return [r for r in map(json.loads, f) if r["branch"] == "main"]

    def restore(self, step, dest):
        # Replay the log from the start up to the step, then write the result.
        state = {}
        for rec in self._main_records():
            if rec["step"] > step:
                break
            state.update(rec["files"])
        write_files(dest, state)

    def who_wrote(self, path, line):
        # Scan the whole log, newest record first, for versions of the file.
        recs = [r for r in reversed(self._main_records()) if path in r["files"]
                and r["step"] != MERGE_STEP]
        return scan_back(((r["step"], r["files"][path]) for r in recs), line)

    def disk(self):
        return self.log.stat().st_size


class SqliteBlobs:
    name = "sqlite blobs"

    def __init__(self, root):
        self.root = Path(root)
        self.work_dir = self.root / "work"
        self.work_dir.mkdir(parents=True)
        self.db_path = self.root / "store.db"
        self.cur = "main"
        self.cur_manifest = {}   # path -> step that holds the blob
        self.manifests = {}      # step -> manifest, for branch and merge
        self.last_step = {}

    def setup(self, project):
        self.db = sqlite3.connect(self.db_path)
        self.db.executescript(
            "create table blobs(step integer, path text, data blob);"
            "create table manifest(step integer, path text, blob_step integer);"
            "create index blobs_path on blobs(path, step);"
            "create index manifest_step on manifest(step);")

    def checkpoint(self, step, changed):
        self.db.executemany("insert into blobs values(?,?,?)",
                            [(step, p, t.encode()) for p, t in changed.items()])
        for p in changed:
            self.cur_manifest[p] = step
        self.db.executemany("insert into manifest values(?,?,?)",
                            [(step, p, s) for p, s in self.cur_manifest.items()])
        self.db.commit()
        self.manifests[step] = dict(self.cur_manifest)
        self.last_step[self.cur] = step

    def branch(self, base_step, name):
        self.cur = name
        rows = self.db.execute("select path, blob_step from manifest where step=?", (base_step,))
        self.cur_manifest = dict(rows.fetchall())

    def merge(self, winner):
        # The winner's blobs are already stored; only a manifest is written.
        self.cur = "main"
        self.cur_manifest = dict(self.manifests[self.last_step[winner]])
        self.db.executemany("insert into manifest values(?,?,?)",
                            [(MERGE_STEP, p, s) for p, s in self.cur_manifest.items()])
        self.db.commit()
        self.manifests[MERGE_STEP] = dict(self.cur_manifest)

    def restore(self, step, dest):
        rows = self.db.execute(
            "select m.path, b.data from manifest m join blobs b "
            "on b.path=m.path and b.step=m.blob_step where m.step=?", (step,))
        for p, data in rows:
            fp = Path(dest) / p
            fp.parent.mkdir(parents=True, exist_ok=True)
            fp.write_bytes(data)

    def who_wrote(self, path, line):
        rows = self.db.execute(
            "select step, data from blobs where path=? and step<? and step<10000 "
            "order by step desc", (path, MERGE_STEP))
        return scan_back(((s, d.decode()) for s, d in rows), line)

    def disk(self):
        return self.db_path.stat().st_size


ADAPTERS = [Velo52, Velo, ShadowGit, GitWorktree, DirCopy, JsonlLog, SqliteBlobs]


# ------------------------------------------------------------------ driver

def tool_version(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True).stdout.strip()
    except OSError:
        return "unavailable"


def bench_system(cls, root, args, project, edits, who_targets, expect_who):
    a = cls(root)
    res = {"system": a.name, "provenance": PROVENANCE[a.name]}
    paths = sorted(project)
    a.setup(project)
    content = dict(project)

    # Workload 1: linear steps. Driver-side file writes are not timed.
    write_files(a.work_dir, project)
    t_total = 0.0
    t0 = time.perf_counter()
    a.checkpoint(0, dict(project))
    t_total += time.perf_counter() - t0
    for step, changed in enumerate(edits, start=1):
        flat = {p: t for p, (t, _l) in changed.items()}
        write_files(a.work_dir, flat)
        content.update(flat)
        t0 = time.perf_counter()
        a.checkpoint(step, flat)
        t_total += time.perf_counter() - t0
    nsteps = len(edits)
    res["steps_s"] = t_total
    res["disk_after_steps"] = a.disk()

    # Workload 3: restore to step n, repeated.
    target = nsteps // 2
    dest = Path(root) / "restore"
    times = []
    for _ in range(args.restores):
        if dest.exists():
            shutil.rmtree(dest)
        dest.mkdir()
        t0 = time.perf_counter()
        a.restore(target, dest)
        times.append(time.perf_counter() - t0)
    res["restore_total_s"] = sum(times)
    res["restore_median_s"] = sorted(times)[len(times) // 2]
    n_files = sum(1 for f in dest.rglob("*") if f.is_file())
    assert n_files == len(project), f"{a.name}: restored {n_files} files"

    # Workload 4: who wrote this line, on the final state of the main line.
    t0 = time.perf_counter()
    got = [a.who_wrote(p, n) for p, n in who_targets]
    res["who_wrote_s"] = time.perf_counter() - t0
    assert got == expect_who, f"{a.name}: who_wrote mismatch {got} vs {expect_who}"

    # Workload 2: branch per attempt from the final main state; the last wins.
    t_branch = 0.0
    winner = None
    for k in range(ATTEMPTS):
        name = f"attempt-{k}"
        arng = random.Random(args.seed * 1000 + k)
        t0 = time.perf_counter()
        a.branch(nsteps, name)
        t_branch += time.perf_counter() - t0
        cur = dict(content)
        write_files(a.work_dir, cur)   # untimed: base state in the attempt's directory
        for i in range(ATTEMPT_STEPS):
            flat = {p: t for p, (t, _l) in make_edit(arng, paths, cur, f"a{k}s{i}").items()}
            cur.update(flat)
            write_files(a.work_dir, flat)
            t0 = time.perf_counter()
            a.checkpoint(10_000 + k * 100 + i, flat)
            t_branch += time.perf_counter() - t0
        winner = name
    t0 = time.perf_counter()
    a.merge(winner)
    t_branch += time.perf_counter() - t0
    res["branches_s"] = t_branch
    res["disk_final"] = a.disk()
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--steps", type=int, default=500)
    ap.add_argument("--files", type=int, default=1000)
    ap.add_argument("--restores", type=int, default=RESTORES)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--quick", action="store_true", help="50 steps, 100 files, 10 restores")
    ap.add_argument("--only", help="comma-separated system names to run")
    ap.add_argument("--workdir", default=None, help="default: a fresh temp directory, removed at the end")
    ap.add_argument("--out", default=None)
    ap.add_argument("--defender-note", default=None,
                    help="Windows: real-time protection state and exclusions")
    args = ap.parse_args()
    if args.quick:
        args.steps, args.files, args.restores = 50, 100, 10

    rng = random.Random(args.seed)
    project = make_project(rng, args.files)
    paths = sorted(project)
    # Generate the linear edits, tracking which step last wrote each line so
    # every system's who_wrote answer can be checked.
    content = dict(project)
    last_writer = {(p, n): 0 for p in paths for n in range(LINES_PER_FILE)}
    edits = []
    for step in range(1, args.steps + 1):
        ch = make_edit(rng, paths, content, step)
        for p, (t, lines) in ch.items():
            content[p] = t
            for n in lines:
                last_writer[(p, n)] = step
        edits.append(ch)
    who_targets = rng.sample(sorted(last_writer), WHO_QUERIES)
    expect_who = [last_writer[t] for t in who_targets]

    work = Path(args.workdir) if args.workdir else Path(tempfile.mkdtemp(prefix="velo-agent-bench-"))
    work.mkdir(parents=True, exist_ok=True)

    import velo
    env = {
        "date": datetime.date.today().isoformat(),
        "platform": platform.platform(),
        "system": platform.system(),
        "machine": platform.machine(),
        "python": sys.version.split()[0],
        "git": tool_version(["git", "--version"]),
        "sqlite": sqlite3.sqlite_version,
        "velo_binding": getattr(velo, "__version__", None) or "unversioned binding",
        "repo_commit": tool_version(["git", "-C", str(Path(__file__).resolve().parent), "rev-parse", "--short", "HEAD"]),
        "workdir": str(work),
        "steps": args.steps, "files": args.files, "restores": args.restores, "seed": args.seed,
        "quick": args.quick,
    }
    if platform.system() == "Windows":
        env["defender_note"] = args.defender_note or "not provided: assume real-time scanning is ON"
    only = set(args.only.split(",")) if args.only else None
    results = []
    for cls in ADAPTERS:
        if only and cls.name not in only:
            continue
        root = work / cls.name.replace(" ", "_")
        shutil.rmtree(root, ignore_errors=True)
        root.mkdir(parents=True)
        print(f"== {cls.name}", file=sys.stderr, flush=True)
        results.append(bench_system(cls, root, args, project, edits, who_targets, expect_who))

    md = ["| system | %d steps (s) | 8 attempts + merge (s) | restore, median (ms) | who wrote x%d (s) | disk (MB) | provenance |"
          % (args.steps, WHO_QUERIES), "|---|---|---|---|---|---|---|"]
    for r in results:
        md.append("| %s | %.1f | %.2f | %.1f | %.2f | %.1f | %s |" % (
            r["system"], r["steps_s"], r["branches_s"], r["restore_median_s"] * 1000,
            r["who_wrote_s"], r["disk_final"] / 1e6, r["provenance"]))
    print("\n".join(md))

    path = args.out or f"bench/results/agent-{env['date']}-{env['system'].lower()}.json"
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps({"env": env, "results": results}, indent=2))
    print(f"wrote {path}", file=sys.stderr)
    if not args.workdir:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
