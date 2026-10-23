# Releasing velo

For maintainers. **Nothing publishes automatically.** Pushing a tag builds and
attaches release assets on GitHub; every registry, tap, bucket, image and
directory below is published by hand with the command shown. The names are in
[NAMES.md](NAMES.md).

## Checklist

Open the next version first, then release it.

1. Set the version in `[workspace.package]` in `Cargo.toml`.
2. Refresh the four lockfiles: `Cargo.lock`, `bindings/node/Cargo.lock`,
   `bindings/python/Cargo.lock` and `bindings/wasm/Cargo.lock`
   (`cargo update -w`, or `cargo build`, in each directory).
3. Align the binding versions: `bindings/node/package.json` and
   `crates/velo-mcp/server.json` carry the same version. The Python wheel takes
   its version from Cargo.
4. Move the semver baseline: `baseline-rev` in the `semver` job of
   `.github/workflows/ci.yml` must name the newest released tag.
5. Check the dry runs are green: the `package` job in `ci.yml`
   (`cargo package --workspace`) and `.github/workflows/publish-dry-run.yml`
   (`cargo publish --dry-run -p velo-merge`, the PyPI sdist and wheel with
   `twine check`, and `npm publish --dry-run` with an install test).
6. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`.

`release.yml` then produces, on the GitHub release: the CLI archives for each
platform, the `velo-mcp` archives, the wheels and Node tarballs from
`packages.yml`, the SBOM, and the checksum files (CLI assets only). Verify them
as described in [INSTALL.md](INSTALL.md).

The release is created as a **draft** first, and every job attaches to that
draft. Only the final `publish` job, which needs `build`, `sbom`,
`attach-bindings` and `checksums`, sets `draft=false`, so users never see a
release with assets missing. A release made with `GITHUB_TOKEN` fires no
`release: published` event, so `release.yml` itself calls `distribution.yml`
(with the tag) and `setup-velo.yml` after `publish`. `setup-velo.yml` does not
run on tag pushes. If a run fails before `publish`, the draft stays; rerun the
failed jobs (uploads use `--clobber`) or delete the draft and the tag.

## Verify a release

`release.yml` calls `release-verify.yml` after `publish`. It checks that every
expected asset is attached (the list lives in `packaging/release_assets.py`),
that the checksums and attestations verify with the commands in
[INSTALL.md](INSTALL.md), and that each asset class installs and runs on a
runner with no Rust toolchain. To check any release by hand:

```bash
gh workflow run release-verify.yml -f tag=vX.Y.Z
```

## Channels

### crates.io

Log in once with `cargo login`. Publish in dependency order, waiting for each
crate to appear in the index before the next (a few seconds to a minute):

```bash
cargo publish --locked -p velo-merge     # no workspace dependencies
cargo publish --locked -p velo-core      # needs velo-merge
cargo publish --locked -p velo-tui       # needs velo-core
cargo publish --locked -p velo-testkit   # needs velo-core
cargo publish --locked -p velo-cli       # needs velo-core and velo-merge
```

`velo-ffi` and `velo-mcp` have `publish = false` and are not published.

### PyPI

Package `velo-core`, from `bindings/python`. Either a token:

```bash
cd bindings/python
MATURIN_PYPI_TOKEN=pypi-... maturin publish
```

or trusted publishing, configured on pypi.org for this repository, with
`pypa/gh-action-pypi-publish` in a workflow of your own. `maturin publish`
builds for the current platform only; to cover all three, upload the wheels
attached to the release with `twine upload`.

### npm

Package `velo-core`, from `bindings/node`. `package.json` has `"private": true`
as a guard; lift it for the publish (`npm pkg set private=false`, and do not
commit that). The package must contain the native `.node` file, so build it
first:

```bash
cd bindings/node
npm install && npx napi build --platform --release
npm publish --access public
```

### Homebrew, scoop and winget

Generated from the published release by `packaging/generate_manifests.py`; see
[packaging/README.md](../packaging/README.md) for the submission steps.

```bash
mkdir assets
gh release download vX.Y.Z --dir assets --pattern 'velo-aarch64-*' --pattern 'velo-x86_64-*'
python3 packaging/generate_manifests.py --tag vX.Y.Z --assets assets --out manifests
```

### Containers

Run `containers/smoke.sh`, then log in and push as written in
[containers/README.md](../containers/README.md):

```sh
echo "$GITHUB_TOKEN" | docker login ghcr.io -u lucasvascovici --password-stdin
```

followed by the tag and push loop in that file.

### MCP directory

The entry is `crates/velo-mcp/server.json`. Once its version matches the
release, submit it with the MCP registry publisher:

```bash
cd crates/velo-mcp
mcp-publisher login github && mcp-publisher publish
```

### GitHub Action

`setup-velo` is used straight from the tag
(`uses: LucasVascovici/velo/setup-velo@vX.Y.Z`), so the tag is the publish.
Update the version in the examples of
[setup-velo/README.md](../setup-velo/README.md).

### Docs site

Set the repository variable `DOCS_PAGES` to `true`; the deploy job in
`docs-site.yml` then runs on the next push to main:

```bash
gh variable set DOCS_PAGES --body true
```
