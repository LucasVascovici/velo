# Package-manager manifests

`generate_manifests.py` renders a Homebrew formula, a scoop manifest and winget
manifests from the archives of a published release. Each manifest points at the
release asset URL and carries the SHA-256 of the archive it was made from. The
ids (`velody`, `LucasVascovici.Velody`) are the ones in `docs/NAMES.md`.

## Regenerate

```bash
mkdir assets
gh release download v6.1.0 --dir assets --pattern 'velody-aarch64-*' --pattern 'velody-x86_64-*'
python3 packaging/generate_manifests.py --tag v6.1.0 --assets assets --out manifests
python3 -m unittest discover packaging      # the generator's own test
```

Output: `manifests/homebrew/velody.rb`, `manifests/scoop/velody.json` and
`manifests/winget/manifests/l/LucasVascovici/Velody/<version>/*.yaml`. The
templates are in `packaging/templates/`.

## CI

`.github/workflows/distribution.yml` runs on `release: published` and by hand
(`workflow_dispatch`, input `tag`). It uploads the `manifests` artifact, then
installs from the formula (`brew install` from a local tap), from the scoop
manifest (`scoop install <file>`), and runs `velody --version` against the tag.
For winget it checks all three YAML files against the published JSON schemas in
microsoft/winget-cli (always), and additionally runs `winget validate
--manifest` when the runner has winget. Hosted runners often do not, so the
schema check is the one that counts. A real winget install is not tested: the
winget sandbox is not available on hosted runners.

Nothing is submitted anywhere by CI.

## Submitting (the owner's steps)

Homebrew, a tap repository:

```bash
gh repo create LucasVascovici/homebrew-velody --public --clone
cd homebrew-velody && mkdir Formula && cp ../manifests/homebrew/velody.rb Formula/
git add Formula/velody.rb && git commit -m "velody 6.1.0" && git push
# users: brew install LucasVascovici/velody/velody
```

scoop, a bucket repository:

```bash
gh repo create LucasVascovici/scoop-velody --public --clone
cd scoop-velody && mkdir bucket && cp ../manifests/scoop/velody.json bucket/
git add bucket/velody.json && git commit -m "velody 6.1.0" && git push
# users: scoop bucket add velody https://github.com/LucasVascovici/scoop-velody
#        scoop install velody
```

winget, either with wingetcreate or a pull request to microsoft/winget-pkgs:

```bash
wingetcreate submit manifests/winget/manifests/l/LucasVascovici/Velody/6.1.0
# or: fork microsoft/winget-pkgs, copy that directory to the same path under
# manifests/, and open a pull request
```
