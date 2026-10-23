#!/usr/bin/env python3
"""Prepare one isolated cold-start sandbox.

    prepare.py --tag vX.Y.Z --wedge {python,node,rust,mcp} --run N --root DIR [--repo PATH]

Builds <root>/<wedge>-run<N>/ with:

  work/        the agent's empty working directory, holding only the docs
               handed over (README.md, docs/LIBRARY.md, the wedge's cookbook),
               copied as text from the tag with `git show`
  assets/      the release files, downloaded with `gh` and verified against
               SHA256SUMS*.txt before anything is installed
  venv/ , work/node_modules , bin/   the installed package, per wedge
  PROMPT.txt   the task card's sentence plus the actor line
  run.json     tag, wedge, run, sandbox, actor, file

Standard library only. Refuses a --root inside a git work tree: an agent in a
checkout can read the source, which invalidates the run.
"""
import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tarfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
GH_REPO = "LucasVascovici/velo"

# Documents handed over per wedge, besides README.md and docs/LIBRARY.md (the
# mapping table in README.md). The MCP wedge gets no cookbook.
COOKBOOK = {
    "python": "docs/cookbook/agent-checkpointing.md",
    "node": "docs/cookbook/config-registry.md",
    "rust": "docs/cookbook/document-editor.md",
    "mcp": None,
}
CARD = {
    "python": "python-agent-checkpointing.md",
    "node": "node-config-registry.md",
    "rust": "rust-document-editor.md",
    "mcp": "mcp-client.md",
}
TERMS = ["save_changes", "velo report", "velo sync"]


def inside_git_tree(path):
    """True when path or any parent holds a .git entry."""
    cur = os.path.abspath(path)
    while True:
        if os.path.exists(os.path.join(cur, ".git")):
            return True
        parent = os.path.dirname(cur)
        if parent == cur:
            return False
        cur = parent


def term_counts(text):
    return {t: text.count(t) for t in TERMS}


def parse_card(path):
    """Return (sentence, actor_line, actor, file) from a task card.

    The card marks the prompt with a `## Task` heading: the first paragraph
    below it is the sentence, and the paragraph that starts "Use the actor
    name" or "Use the run id" names the actor. The file comes from the
    `--file X` of the checker command; a `<placeholder>` means none is fixed.
    """
    with open(path, encoding="utf-8") as f:
        text = f.read()
    m = re.search(r"^## Task[^\n]*\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    if not m:
        raise SystemExit("no '## Task' section in " + path)
    paras = [" ".join(p.split()) for p in re.split(r"\n\s*\n", m.group(1).strip()) if p.strip()]
    if len(paras) < 2:
        raise SystemExit("task card needs a sentence and an actor paragraph: " + path)
    sentence = paras[0]
    actor_line = re.sub(r"\s*\(Give.*?\)\s*$", "", paras[1])
    am = re.search(r"`([^`]+)`", actor_line)
    actor = am.group(1) if am else ""
    fm = re.search(r"--file\s+(\S+)", text)
    file = fm.group(1) if fm and not fm.group(1).startswith("<") else ""
    return sentence, actor_line, actor, file


def target():
    """The release target name for this machine, e.g. x86_64-linux."""
    mach = platform.machine().lower()
    arch = "aarch64" if mach in ("arm64", "aarch64") else "x86_64"
    if sys.platform.startswith("linux"):
        return arch + "-linux", "linux"
    if sys.platform == "darwin":
        return arch + "-macos", "macos"
    return "x86_64-windows", "windows"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def verify(assets_dir, files):
    """Check every file against the SHA256SUMS*.txt files in assets_dir.

    A file listed in no sums file, or with a different digest, stops the run.
    """
    sums = {}
    for name in os.listdir(assets_dir):
        if re.fullmatch(r"SHA256SUMS.*\.txt", name):
            with open(os.path.join(assets_dir, name), encoding="utf-8") as f:
                for line in f:
                    parts = line.split()
                    if len(parts) == 2:
                        sums[os.path.basename(parts[1].lstrip("*"))] = parts[0].lower()
    for name in files:
        if name not in sums:
            raise SystemExit("checksum: %s is not listed in any SHA256SUMS file" % name)
        got = sha256(os.path.join(assets_dir, name))
        if got != sums[name]:
            raise SystemExit("checksum MISMATCH for %s: expected %s, got %s" % (name, sums[name], got))
        print("verified " + name)


def download(tag, assets_dir, patterns):
    cmd = ["gh", "release", "download", tag, "--repo", GH_REPO, "--dir", assets_dir, "--clobber"]
    for p in patterns:
        cmd += ["--pattern", p]
    subprocess.run(cmd, check=True)


def pick(assets_dir, pattern):
    import fnmatch

    hits = sorted(n for n in os.listdir(assets_dir) if fnmatch.fnmatch(n, pattern))
    if len(hits) != 1:
        raise SystemExit("expected one asset matching %s, found %s" % (pattern, hits))
    return hits[0]


def extract(archive, dest):
    os.makedirs(dest, exist_ok=True)
    if archive.endswith(".zip"):
        with zipfile.ZipFile(archive) as z:
            z.extractall(dest)
    else:
        with tarfile.open(archive) as t:
            try:
                t.extractall(dest, filter="data")
            except TypeError:  # Python before the extraction filters
                t.extractall(dest)


def git_show(repo, tag, path):
    r = subprocess.run(
        ["git", "-C", repo, "show", "%s:%s" % (tag, path)],
        capture_output=True,
        check=True,
    )
    return r.stdout.decode("utf-8")


def run(cmd, **kw):
    subprocess.run(cmd, check=True, **kw)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tag", required=True)
    ap.add_argument("--wedge", required=True, choices=sorted(CARD))
    ap.add_argument("--run", required=True, type=int)
    ap.add_argument("--root", required=True)
    ap.add_argument("--repo", default=DEFAULT_REPO)
    a = ap.parse_args(argv)

    root = os.path.abspath(a.root)
    if inside_git_tree(root):
        print("refusing: --root %s is inside a git work tree" % root, file=sys.stderr)
        return 2
    sandbox = os.path.join(root, "%s-run%d" % (a.wedge, a.run))
    work = os.path.join(sandbox, "work")
    assets = os.path.join(sandbox, "assets")
    if os.path.exists(work):
        print("refusing: %s already exists; every run gets a fresh sandbox" % sandbox, file=sys.stderr)
        return 2
    os.makedirs(work)
    os.makedirs(assets, exist_ok=True)

    # 1. The documents, as text, from the tag.
    handed = ["README.md", "docs/LIBRARY.md"]
    if COOKBOOK[a.wedge]:
        handed.append(COOKBOOK[a.wedge])
    counts = {}
    for rel in handed:
        text = git_show(a.repo, a.tag, rel)
        dest = os.path.join(work, *rel.split("/"))
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        with open(dest, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        counts[rel] = term_counts(text)

    # 2. Release assets, verified, then installed.
    ver = a.tag.lstrip("v")
    tgt, osname = target()
    if a.wedge == "python":
        wheel_pat = {
            "linux": "velo_core-%s-*manylinux*%s*.whl" % (ver, tgt.split("-")[0]),
            "macos": "velo_core-%s-*macosx*%s.whl" % (ver, "arm64" if tgt.startswith("aarch64") else "x86_64"),
            "windows": "velo_core-%s-*win_amd64.whl" % ver,
        }[osname]
        download(a.tag, assets, ["SHA256SUMS-bindings.txt", wheel_pat])
        wheel = pick(assets, wheel_pat)
        verify(assets, [wheel])
        venv = os.path.join(sandbox, "venv")
        run([sys.executable, "-m", "venv", venv])
        py = os.path.join(venv, "Scripts" if os.name == "nt" else "bin", "python")
        run([py, "-m", "pip", "install", "--quiet", os.path.join(assets, wheel)])
    elif a.wedge == "node":
        tgz_pat = "velo-core-%s-%s.tgz" % (ver, osname)
        download(a.tag, assets, ["SHA256SUMS-bindings.txt", tgz_pat])
        verify(assets, [pick(assets, tgz_pat)])
        npm = "npm.cmd" if os.name == "nt" else "npm"
        run([npm, "init", "-y"], cwd=work, stdout=subprocess.DEVNULL)
        run([npm, "install", os.path.join(assets, tgz_pat)], cwd=work)
    elif a.wedge == "mcp":
        ext = "zip" if osname == "windows" else "tar.gz"
        pats = ["velo-%s.%s" % (tgt, ext), "velo-mcp-%s.%s" % (tgt, ext)]
        download(a.tag, assets, ["SHA256SUMS.txt"] + pats)
        verify(assets, pats)
        for p in pats:
            extract(os.path.join(assets, p), os.path.join(sandbox, "bin"))
    # rust: nothing is installed; the agent adds the crate to its own Cargo.toml.

    # 3. The prompt and the run record.
    sentence, actor_line, actor, file = parse_card(os.path.join(HERE, "tasks", CARD[a.wedge]))
    with open(os.path.join(sandbox, "PROMPT.txt"), "w", encoding="utf-8") as f:
        f.write(sentence + "\n\n" + actor_line + "\n")
    with open(os.path.join(sandbox, "run.json"), "w", encoding="utf-8") as f:
        json.dump(
            {"tag": a.tag, "wedge": a.wedge, "run": a.run, "sandbox": sandbox, "actor": actor, "file": file},
            f,
            indent=2,
        )
        f.write("\n")

    # 4. The term check.
    print("\nterm check at %s (count per handed-over file)" % a.tag)
    print("| File | " + " | ".join(TERMS) + " |")
    print("| --- | " + " | ".join("---" for _ in TERMS) + " |")
    for rel, c in counts.items():
        print("| %s | %s |" % (rel, " | ".join(str(c[t]) for t in TERMS)))
    for rel, c in counts.items():
        for t in TERMS:
            if c[t] == 0:
                print("| doc-defect | %s | missing term `%s` |" % (rel, t))
    print("\nsandbox: " + sandbox)
    return 0


if __name__ == "__main__":
    sys.exit(main())
