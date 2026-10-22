#!/usr/bin/env python3
"""The list of assets a velo release must carry, and a checker for it.

The names come from the `build` matrix in .github/workflows/release.yml
(`package` and `mcp_package`), the wheel and tarball renames in
.github/workflows/packages.yml, and docs/RELEASING.md. Keep the list here and
nowhere else; the release-verify workflow reads it from this file.

Usage: release_assets.py --tag vX.Y.Z --assets-json FILE
where FILE is the output of `gh release view TAG --json assets`. Missing names
are an error (exit 1); unexpected names are only a warning.
"""
import argparse
import json
import sys

CLI_ARCHIVES = [
    "x86_64-linux.tar.gz",
    "aarch64-linux.tar.gz",
    "x86_64-macos.tar.gz",
    "aarch64-macos.tar.gz",
    "x86_64-windows.zip",
]

WHEEL_TAGS = [
    "manylinux_2_17_x86_64.manylinux2014_x86_64",
    "macosx_11_0_arm64",
    "win_amd64",
]

NODE_PLATFORMS = ["linux", "macos", "windows"]


def expected(version):
    """Every asset name for `version` (no leading `v`)."""
    names = {f"velo-{a}" for a in CLI_ARCHIVES}
    names |= {f"velo-mcp-{a}" for a in CLI_ARCHIVES}
    names |= {f"velo_core-{version}-cp39-abi3-{t}.whl" for t in WHEEL_TAGS}
    names |= {f"velo-core-{version}-{p}.tgz" for p in NODE_PLATFORMS}
    names |= {
        "SHA256SUMS.txt",
        "SHA256SUMS.txt.sigstore.json",
        "SHA256SUMS-bindings.txt",
        "SHA256SUMS-bindings.txt.sigstore.json",
        f"velo-{version}.cdx.json",
        "api-node.zip",
        "api-python.zip",
    }
    return names


def compare(version, attached):
    """Return (missing, unexpected), both sorted."""
    want = expected(version)
    have = set(attached)
    return sorted(want - have), sorted(have - want)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--tag", required=True)
    p.add_argument("--assets-json", required=True)
    args = p.parse_args(argv)
    with open(args.assets_json, encoding="utf-8") as f:
        attached = [a["name"] for a in json.load(f)["assets"]]
    version = args.tag.lstrip("v")
    missing, extra = compare(version, attached)
    for name in extra:
        print(f"warning: unexpected asset {name}")
    for name in missing:
        print(f"missing: {name}")
    if missing:
        return 1
    print(f"ok: all {len(expected(version))} expected assets are attached")
    return 0


if __name__ == "__main__":
    sys.exit(main())
