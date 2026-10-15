#!/usr/bin/env python3
"""Binding parity check (ARCHITECTURE 18.8).

Enumerates the public embedder entry points of velo-core by parsing its
sources, then asserts every one is either bound in the Python and Node
bindings or listed in exemptions.toml with a reason.  Exit 0 when clean,
1 otherwise.  Stdlib only; see README.md.
"""
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE_SRC = ROOT / "crates" / "velo-core" / "src"
BINDINGS = {
    "python": ROOT / "bindings" / "python" / "src",
    "node": ROOT / "bindings" / "node" / "src",
}
EXEMPTIONS = Path(__file__).resolve().parent / "exemptions.toml"

# Extra core modules whose module-level `pub fn`s are entry points
# (signing is bound since 18.8-d).
EXTRA_MODULES: list[str] = ["sign"]

IMPL_RE = re.compile(r"^impl(?:<[^>]*>)?\s+(Repo|WriteGuard)\b(?!\s+for)")
PUB_FN_RE = re.compile(r"^pub\s+(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)")
STRIP_RE = re.compile(
    r"""//[^\n]*|/\*.*?\*/|r(#*)".*?"\1|"(?:\\.|[^"\\])*"|'(?:\\[^']*|[^\\'])'""",
    re.S,
)


def strip(text: str) -> str:
    """Blank out comments, strings and char literals, keeping newlines."""
    return STRIP_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)


def parse_entries(text: str, module: str | None, methods: bool) -> list[str]:
    """Return entry names found in one source file.

    Module-level fns are named `commands::<module>::<fn>` (or the bare name
    for the given module string); impl methods `Repo::<fn>`/`WriteGuard::<fn>`.
    """
    entries: list[str] = []
    depth = 0
    impl_ty: str | None = None
    impl_depth = 0
    attrs: list[str] = []
    skip_block_depth: int | None = None  # a #[cfg(test)] / hidden impl
    for raw in strip(text).split("\n"):
        line = raw.strip()
        if line.startswith("#["):
            attrs.append(line)
        elif line:
            hidden = any("doc(hidden)" in a or "cfg(test)" in a for a in attrs)
            at_module = depth == 0 and impl_ty is None
            if methods and depth == 0:
                m = IMPL_RE.match(line)
                if m:
                    impl_ty = m.group(1)
                    impl_depth = depth
                    skip_block_depth = None
                    if hidden:
                        impl_ty = "!" + impl_ty
            if at_module and module is not None and not hidden:
                m = PUB_FN_RE.match(line)
                if m:
                    entries.append(f"{module}::{m.group(1)}")
            elif impl_ty and not impl_ty.startswith("!") and depth == impl_depth + 1 and not hidden:
                m = re.match(r"pub\s+(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)", line)
                if m:
                    entries.append(f"{impl_ty}::{m.group(1)}")
            attrs = []
        depth += raw.count("{") - raw.count("}")
        if impl_ty and depth <= impl_depth and "}" in raw:
            impl_ty = None
    del skip_block_depth
    return entries


def core_entries() -> list[str]:
    out: list[str] = []
    for f in sorted((CORE_SRC / "commands").glob("*.rs")):
        # `pub(crate) mod` modules are not public surface.
        mod_rs = (CORE_SRC / "commands" / "mod.rs").read_text("utf-8", "replace")
        if f.stem != "mod" and re.search(rf"^pub\(crate\)\s+mod\s+{f.stem}\b", mod_rs, re.M):
            continue
        name = "commands" if f.stem == "mod" else f"commands::{f.stem}"
        out += parse_entries(f.read_text("utf-8", "replace"), name, False)
    for fname in ("repo.rs", "tree.rs"):
        out += parse_entries((CORE_SRC / fname).read_text("utf-8", "replace"), None, True)
    for mod in EXTRA_MODULES:
        out += parse_entries((CORE_SRC / f"{mod}.rs").read_text("utf-8", "replace"), mod, False)
    return sorted(set(out))


def evidence_regex(entry: str) -> str:
    parts = entry.split("::")
    fn = parts[-1]
    if parts[0] in ("Repo", "WriteGuard"):
        return rf"(?:\.|\w*Repo::|WriteGuard::){fn}\s*\("
    if entry.startswith("commands::") and len(parts) == 2:  # commands/mod.rs
        return rf"\b{fn}\s*\("
    return rf"\b{parts[-2]}::{fn}\s*\("


def binding_sources() -> dict[str, str]:
    return {
        name: "\n".join(
            strip(p.read_text("utf-8", "replace")) for p in sorted(d.rglob("*.rs"))
        )
        for name, d in BINDINGS.items()
    }


def main() -> int:
    entries = core_entries()
    sources = binding_sources()
    cfg = tomllib.loads(EXEMPTIONS.read_text("utf-8"))
    exempt: dict[str, dict[str, str]] = {}
    problems: list[str] = []
    for e in cfg.get("exempt", []):
        reason = (e.get("reason") or "").strip()
        if not reason:
            problems.append(f"exemption {e.get('entry')!r} has no reason")
        for b in e.get("bindings", []):
            if b not in BINDINGS:
                problems.append(f"exemption {e['entry']!r}: unknown binding {b!r}")
            exempt.setdefault(e["entry"], {})[b] = reason
    forced: dict[str, dict[str, str]] = {}
    for e in cfg.get("bound", []):
        forced.setdefault(e["entry"], {})[e["binding"]] = e["evidence"]

    counts = {b: {"bound": 0, "exempt": 0} for b in BINDINGS}
    for entry in entries:
        for b, src in sources.items():
            ev = forced.get(entry, {}).get(b)
            bound = bool(re.search(ev or evidence_regex(entry), src))
            if b in exempt.get(entry, {}):
                counts[b]["exempt"] += 1
                if bound:
                    problems.append(f"stale exemption: {entry} is now bound in {b}")
            elif bound:
                counts[b]["bound"] += 1
            else:
                problems.append(f"unbound: {entry} is neither bound in {b} nor exempt")
    known = set(entries)
    for entry in sorted(exempt):
        if entry not in known:
            problems.append(f"stale exemption: {entry} no longer exists in velo-core")
    for entry in sorted(forced):
        if entry not in known:
            problems.append(f"stale [[bound]] entry: {entry} no longer exists in velo-core")

    print(f"{len(entries)} core entry points")
    print(f"{'binding':<8} {'bound':>6} {'exempt':>7}")
    for b, c in counts.items():
        print(f"{b:<8} {c['bound']:>6} {c['exempt']:>7}")
    if problems:
        print(f"\n{len(problems)} problem(s):")
        for p in problems:
            print(f"  {p}")
        return 1
    print("parity OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
