# Release check: v6.2.0

Checked on 2026-10-23 from a Windows 11 machine (Git Bash, `gh` 2.102.0), which is
not the machine that built the release (CI did). The release was published
2026-10-22T21:43:30Z. Nothing was tagged, re-released, published or submitted.

Release: https://github.com/LucasVascovici/velo/releases/tag/v6.2.0

## Asset completeness

`gh release view v6.2.0 --json assets` then
`python packaging/release_assets.py --tag v6.2.0 --assets-json <file>`:

    ok: all 23 expected assets are attached

| Asset class | Result | Evidence |
| --- | --- | --- |
| All 23 expected assets attached (now) | pass | `release_assets.py`, run on 2026-10-23 |

## Checksums (commands from `docs/INSTALL.md`, run as written)

All 23 assets were downloaded with `gh release download v6.2.0` into a scratch
directory outside the checkout, then `sha256sum -c --ignore-missing <file>`.

| Check | Result | Notes |
| --- | --- | --- |
| `SHA256SUMS.txt` (CLI archives, velo-mcp archives, SBOM) | pass | 11 of 11 files OK |
| `SHA256SUMS-bindings.txt` (wheels, Node tarballs) | pass | 6 of 6 files OK |
| PowerShell `Get-FileHash velo-x86_64-windows.zip` form for Windows | pass | hash `523a90e9...659b` equals the `SHA256SUMS.txt` line (compared with `sha256sum` here; the doc gives the PowerShell form) |
| `cosign verify-blob` on both checksum files | not run | `cosign` is not installed on this machine; owner or the release-verify run covers it |

## Attestations (`gh attestation verify <file> --repo LucasVascovici/velo`)

| Asset | Result | Notes |
| --- | --- | --- |
| `velo-x86_64-linux.tar.gz` (CLI) | pass | exit 0 |
| `velo-mcp-x86_64-linux.tar.gz` (velo-mcp) | pass | exit 0 |
| `velo-x86_64-windows.zip` (CLI, Windows) | pass | exit 0 |

`gh` prints nothing when stdout is not a terminal, so the exit status is the
evidence. No command in `docs/INSTALL.md` failed as written, so the document is
unchanged.

## Asset classes

Presence and checksum only; install results are in the release-verify table.

| Class | Assets | Present | Checksum |
| --- | --- | --- | --- |
| CLI, linux x86_64 / aarch64 | `velo-x86_64-linux.tar.gz`, `velo-aarch64-linux.tar.gz` | pass | pass |
| CLI, macOS x86_64 / aarch64 | `velo-x86_64-macos.tar.gz`, `velo-aarch64-macos.tar.gz` | pass | pass |
| CLI, Windows x86_64 | `velo-x86_64-windows.zip` | pass | pass |
| velo-mcp, 5 platforms | `velo-mcp-*` | pass | pass |
| Python wheels (linux, macOS arm64, Windows) | `velo_core-6.2.0-*.whl` | pass | pass |
| Node tarballs (linux, macOS, Windows) | `velo-core-6.2.0-*.tgz` | pass | pass |
| SBOM | `velo-6.2.0.cdx.json` | pass | pass |

## Generated package-manager manifests

| Job | Result | Notes |
| --- | --- | --- |
| `generate` | not run | see below |
| `brew` | not run | |
| `scoop` | not run | |
| `winget` | not run | |

The dispatch `gh workflow run distribution.yml --ref main -f tag=v6.2.0` was
refused by this session's permission layer (it classes a workflow dispatch as a
production deploy), so it was not attempted again. `distribution.yml` has no run
at all in `gh run list`. Owner command, then
`gh run watch` and read the four jobs; nothing is submitted anywhere:

    gh workflow run distribution.yml --ref main -f tag=v6.2.0

## Clean-machine installs (release-verify)

`release-verify.yml` is on branch `phase-23` only, not main, so it cannot be
dispatched yet. Two push runs exist, both still stuck after about 1.5 hours when
checked (jobs queued or in progress; macOS arm64 capacity was flagged by GitHub).
Run 1: https://github.com/LucasVascovici/velo/actions/runs/39870618921
Run 2: https://github.com/LucasVascovici/velo/actions/runs/39870976394

| Job | Run 1 | Run 2 |
| --- | --- | --- |
| Assets are complete | pass | pass |
| CLI, ubuntu-latest / windows-latest / macos-latest | pass | not finished |
| CLI, ubuntu-24.04-arm | pass | pass |
| Checksums and attestations, ubuntu-latest / macos-latest | pass | not finished |
| Checksums and attestations, windows-latest | not finished | not finished |
| velo-mcp, windows-latest | pass | not finished |
| velo-mcp, ubuntu-latest (step "Initialize and list tools over stdio") | fail | not finished |
| Wheel, linux / macos | pass | linux not finished, macos pass |
| Wheel, windows (step "Download, install into a fresh venv, save and read history") | fail | not finished |
| Node tarball, linux / macos / windows | pass | not finished |

The two failures are from run 1 only; the log is unavailable until the run
completes, so the cause is not recorded here. Run 2 is the commit that pins
Python, which may address the Windows wheel. Once the workflow is on main, the
owner command is:

    gh workflow run release-verify.yml -f tag=v6.2.0

Until that finishes, the clean-machine install rows above that are not "pass"
count as not run.

## Findings

1. The release was visible before its assets were attached. setup-velo run
   39859340191 failed with a 404 because it resolved the new release before the
   archives were uploaded. Fixed in 6.2.1 (release.yml): the release is created
   as a draft and published only when every asset is attached.
2. `distribution.yml` never ran on a release. Events caused by `GITHUB_TOKEN` do
   not trigger other workflows, so the `release: published` trigger never fired.
   Fixed in 6.2.1 (release.yml): the draft-then-publish change runs distribution
   from the release workflow itself.
3. The v6.2.0 release is complete today (23 of 23 assets), every checksum and
   the three attestations checked verify, so the 404 window was transient.
4. `cosign verify-blob` was not exercised here (no cosign installed).
5. The manifest generator has still never been run against the real release; the
   owner dispatch above closes that.
6. release-verify run 1 shows a velo-mcp stdio failure on ubuntu and a wheel
   failure on Windows; both need a look once the logs are available.
