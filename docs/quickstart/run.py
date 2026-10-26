#!/usr/bin/env python3
"""Run the code blocks of the quickstart pages from built packages.

A quickstart that rots should fail a build. Each page in this directory marks
the blocks that must work with an info string ending in `quickstart`:

    ```python quickstart
    ```

This script pulls those blocks out, writes them into a fresh temporary
directory and runs them against the packages a stranger would install, never
against the source tree:

    python   a venv with the wheel installed (`pip install <wheel>`)
    js       an empty directory with the tarball installed (`npm install <tgz>`)
    rust     `cargo new`, with velody-core taken from this checkout through a
             [patch] section, then `cargo run`
    mcp      a built `velody-mcp`, named in $VELODY_MCP, driven over stdio by the
             page's own python block

Only the standard library is used. The exit status is non-zero when any page
fails, and the failing pages are named on the last line.

    python docs/quickstart/run.py --wheel w.whl --tgz p.tgz --velody-mcp velody-mcp
    python docs/quickstart/run.py --self-test
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent

# page file -> the runner that knows how to run its blocks.
PAGES = {
    "python-agent-checkpointing.md": "python",
    "node-config-registry.md": "node",
    "rust-document-editor.md": "rust",
    "mcp.md": "mcp",
}

FENCE = re.compile(r"^```(\S*)[ \t]*(.*)$")


def extract(text: str) -> list[tuple[str, str]]:
    """The (language, code) of every fenced block marked `quickstart`."""
    blocks: list[tuple[str, str]] = []
    lang = None  # the open fence's language when it is a marked one
    inside = False
    body: list[str] = []
    for line in text.splitlines():
        m = FENCE.match(line)
        if not inside:
            if m:
                inside = True
                words = m.group(2).split()
                lang = m.group(1) if "quickstart" in words else None
                body = []
        elif line.startswith("```"):
            if lang is not None:
                blocks.append((lang, "\n".join(body) + "\n"))
            inside = False
            lang = None
        else:
            body.append(line)
    if inside:
        raise ValueError("a code fence is never closed")
    return blocks


def join(blocks: list[tuple[str, str]], lang: str) -> str:
    """Every block of `lang`, in order, as one program."""
    code = "".join(c for l, c in blocks if l == lang)
    if not code:
        raise ValueError(f"no `{lang} quickstart` block")
    return code


def run(cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True, timeout=1500)


def venv_python(venv: Path) -> Path:
    scripts = venv / ("Scripts" if os.name == "nt" else "bin")
    return scripts / ("python.exe" if os.name == "nt" else "python")


def run_python(blocks, work: Path, args) -> None:
    wheel = need(args.wheel, "--wheel")
    run([args.python, "-m", "venv", "venv"], work)
    py = str(venv_python(work / "venv"))
    run([py, "-m", "pip", "install", "--only-binary=:all:", str(wheel)], work)
    (work / "quickstart.py").write_text(join(blocks, "python"), encoding="utf-8")
    run([py, "quickstart.py"], work)


def run_node(blocks, work: Path, args) -> None:
    tgz = need(args.tgz, "--tgz")
    run([args.npm, "init", "-y"], work)
    run([args.npm, "install", str(tgz)], work)
    (work / "quickstart.mjs").write_text(join(blocks, "js"), encoding="utf-8")
    run([args.node, "quickstart.mjs"], work)


def run_rust(blocks, work: Path, args) -> None:
    run([args.cargo, "new", "--bin", "--name", "quickstart", "app"], work)
    app = work / "app"
    version = workspace_version(args.checkout)
    crates = args.checkout / "crates"
    manifest = (app / "Cargo.toml").read_text(encoding="utf-8")
    manifest += (
        f'velody-core = "{version}"\n\n'
        "[patch.crates-io]\n"
        f'velody-core = {{ path = "{(crates / "velody-core").as_posix()}" }}\n'
        f'velody-merge = {{ path = "{(crates / "velody-merge").as_posix()}" }}\n'
    )
    (app / "Cargo.toml").write_text(manifest, encoding="utf-8")
    (app / "src" / "main.rs").write_text(join(blocks, "rust"), encoding="utf-8")
    run([args.cargo, "run", "--quiet"], app)


def run_mcp(blocks, work: Path, args) -> None:
    server = need(args.velody_mcp, "--velody-mcp")
    env = dict(os.environ, VELODY_MCP=str(server))
    (work / "quickstart.py").write_text(join(blocks, "python"), encoding="utf-8")
    run([args.python, "quickstart.py"], work, env)


RUNNERS = {"python": run_python, "node": run_node, "rust": run_rust, "mcp": run_mcp}


def need(value, flag: str):
    if value is None:
        raise ValueError(f"{flag} is needed for this page")
    # The page runs in another directory, so a path that exists is made absolute;
    # a bare name is left for the shell's PATH lookup.
    p = Path(value)
    return p.resolve() if p.exists() else value


def workspace_version(checkout: Path) -> str:
    text = (checkout / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        raise ValueError("no workspace version in Cargo.toml")
    return m.group(1)


def run_page(page: str, args) -> None:
    blocks = extract((HERE / page).read_text(encoding="utf-8"))
    if not blocks:
        raise ValueError("the page has no `quickstart` block")
    work = Path(tempfile.mkdtemp(prefix="velody-quickstart-"))
    try:
        RUNNERS[PAGES[page]](blocks, work, args)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def self_test() -> int:
    """The extractor, and the rule that a broken block fails the run."""
    page = (
        "# t\n\n```sh\nnot run\n```\n\n"
        "```python quickstart\nprint(1)\n```\n\n"
        "```python\nunmarked()\n```\n\n"
        "```js quickstart\nconsole.log(2)\n```\n"
    )
    assert extract(page) == [("python", "print(1)\n"), ("js", "console.log(2)\n")]
    try:
        extract("```python quickstart\nx = 1\n")
    except ValueError:
        pass
    else:
        raise AssertionError("an unclosed fence must be an error")
    broken = extract("```python quickstart\nraise SystemExit(3)\n```\n")
    work = Path(tempfile.mkdtemp(prefix="velody-quickstart-selftest-"))
    try:
        (work / "q.py").write_text(join(broken, "python"), encoding="utf-8")
        failed = False
        try:
            run([sys.executable, "q.py"], work)
        except subprocess.CalledProcessError:
            failed = True
        assert failed, "a broken block must make the run fail"
    finally:
        shutil.rmtree(work, ignore_errors=True)
    # Every page of the suite has a block to run.
    for page_name in PAGES:
        assert extract((HERE / page_name).read_text(encoding="utf-8")), page_name
    print("self-test ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--wheel", help="the built abi3 wheel (python page)")
    ap.add_argument("--tgz", help="the packed Node tarball (node page)")
    ap.add_argument("--velody-mcp", help="a built velody-mcp binary (mcp page)")
    ap.add_argument("--checkout", type=Path, default=ROOT,
                    help="velody checkout the rust page takes velody-core from (default: this one)")
    ap.add_argument("--page", action="append", choices=sorted(PAGES),
                    help="run only this page; repeat for several (default: all)")
    ap.add_argument("--python", default=sys.executable, help="interpreter (default: this one)")
    ap.add_argument("--node", default="node")
    ap.add_argument("--npm", default="npm.cmd" if os.name == "nt" else "npm")
    ap.add_argument("--cargo", default="cargo")
    ap.add_argument("--self-test", action="store_true",
                    help="check the extractor and the failure path, then exit")
    args = ap.parse_args()
    if args.self_test:
        return self_test()

    failed = []
    for page in args.page or list(PAGES):
        print(f"::group::{page}", flush=True)
        try:
            run_page(page, args)
        except (subprocess.SubprocessError, ValueError, OSError) as e:
            print(f"FAILED {page}: {e}", file=sys.stderr, flush=True)
            failed.append(page)
        print("::endgroup::", flush=True)
    if failed:
        print("quickstart pages failed: " + ", ".join(failed), file=sys.stderr)
        return 1
    print("all quickstart pages passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
