#!/usr/bin/env python3
"""Render Homebrew, scoop and winget manifests from a velo release.

Usage: generate_manifests.py --tag vX.Y.Z --assets <dir> --out <dir>

The assets directory holds the release archives (see ASSETS). Each manifest
points at the public release URL and carries the SHA-256 of the archive found
in the directory, so a manifest can never describe bytes it was not made from.
Nothing is uploaded or submitted anywhere.
"""
import argparse
import hashlib
import re
import sys
from pathlib import Path

REPO_URL = "https://github.com/LucasVascovici/velo"
DESCRIPTION = "A fast, safe, intuitive version control system."
WINGET_ID = "LucasVascovici.Velo"
# Current winget manifest schema version.
WINGET_SCHEMA = "1.6.0"
HERE = Path(__file__).resolve().parent

# Placeholder key suffix -> release asset name.
ASSETS = {
    "AARCH64_MACOS": "velo-aarch64-macos.tar.gz",
    "X86_64_MACOS": "velo-x86_64-macos.tar.gz",
    "AARCH64_LINUX": "velo-aarch64-linux.tar.gz",
    "X86_64_LINUX": "velo-x86_64-linux.tar.gz",
    "X86_64_WINDOWS": "velo-x86_64-windows.zip",
}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def render(template, values):
    text = template.read_text(encoding="utf-8")
    for key, val in values.items():
        text = text.replace("@@%s@@" % key, val)
    left = re.findall(r"@@[A-Z0-9_]+@@", text)
    if left:
        raise SystemExit("%s: unfilled placeholders %s" % (template.name, left))
    return text


def values_for(tag, assets_dir):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise SystemExit("tag must look like v1.2.3, got %r" % tag)
    values = {
        "VERSION": tag[1:],
        "DESCRIPTION": DESCRIPTION,
        "HOMEPAGE": REPO_URL,
        "REPO_URL": REPO_URL,
        "WINGET_ID": WINGET_ID,
        "SCHEMA": WINGET_SCHEMA,
    }
    for key, name in ASSETS.items():
        path = Path(assets_dir) / name
        if not path.is_file():
            raise SystemExit("missing release asset %s in %s" % (name, assets_dir))
        digest = sha256(path)
        values["URL_" + key] = "%s/releases/download/%s/%s" % (REPO_URL, tag, name)
        values["SHA_" + key] = digest
        values["SHA_%s_UPPER" % key] = digest.upper()
    return values


def generate(tag, assets_dir, out_dir):
    """Write all manifests and return the list of written paths."""
    values = values_for(tag, assets_dir)
    out = Path(out_dir)
    plan = [
        (HERE / "templates/velo.rb.in", out / "homebrew/velo.rb"),
        (HERE / "templates/velo.scoop.json.in", out / "scoop/velo.json"),
    ]
    wdir = out / "winget" / "manifests" / "l" / "LucasVascovici" / "Velo" / values["VERSION"]
    for tpl in sorted((HERE / "templates/winget").glob("*.yaml.in")):
        plan.append((tpl, wdir / tpl.name[: -len(".in")]))
    written = []
    for tpl, dest in plan:
        dest.parent.mkdir(parents=True, exist_ok=True)
        # newline="\n" keeps the files byte-identical across runners.
        with open(dest, "w", encoding="utf-8", newline="\n") as f:
            f.write(render(tpl, values))
        written.append(dest)
    return written


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--tag", required=True)
    ap.add_argument("--assets", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    for p in generate(args.tag, args.assets, args.out):
        print(p)
    return 0


if __name__ == "__main__":
    sys.exit(main())
