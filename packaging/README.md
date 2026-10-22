# Package-manager manifests

`generate_manifests.py` renders a Homebrew formula, a scoop manifest and winget
manifests from the archives of a published release. Each manifest points at the
release asset URL and carries the SHA-256 of the archive it was made from. The
ids (`velo`, `LucasVascovici.Velo`) are the ones in `docs/NAMES.md`.

## Regenerate

```bash
mkdir assets
gh release download v6.1.0 --dir assets --pattern 'velo-aarch64-*' --pattern 'velo-x86_64-*'
python3 packaging/generate_manifests.py --tag v6.1.0 --assets assets --out manifests
python3 -m unittest discover packaging      # the generator's own test
```

Output: `manifests/homebrew/velo.rb`, `manifests/scoop/velo.json` and
`manifests/winget/manifests/l/LucasVascovici/Velo/<version>/*.yaml`. The
templates are in `packaging/templates/`.

## CI

`.github/workflows/distribution.yml` runs on `release: published` and by hand
(`workflow_dispatch`, input `tag`). It uploads the `manifests` artifact, then
installs from the formula (`brew install` from a local tap), from the scoop
manifest (`scoop install <file>`), and runs `velo --version` against the tag.
For winget it checks all three YAML files against the published JSON schemas in
microsoft/winget-cli (always), and additionally runs `winget validate
--manifest` when the runner has winget. Hosted runners often do not, so the
schema check is the one that counts. A real winget install is not tested: the
winget sandbox is not available on hosted runners.

Nothing is submitted anywhere by CI.

## Submitting (the owner's steps)

Homebrew, a tap repository:

```bash
gh repo create LucasVascovici/homebrew-velo --public --clone
cd homebrew-velo && mkdir Formula && cp ../manifests/homebrew/velo.rb Formula/
git add Formula/velo.rb && git commit -m "velo 6.1.0" && git push
# users: brew install LucasVascovici/velo/velo
```

scoop, a bucket repository:

```bash
gh repo create LucasVascovici/scoop-velo --public --clone
cd scoop-velo && mkdir bucket && cp ../manifests/scoop/velo.json bucket/
git add bucket/velo.json && git commit -m "velo 6.1.0" && git push
# users: scoop bucket add velo https://github.com/LucasVascovici/scoop-velo
#        scoop install velo
```

winget, either with wingetcreate or a pull request to microsoft/winget-pkgs:

```bash
wingetcreate submit manifests/winget/manifests/l/LucasVascovici/Velo/6.1.0
# or: fork microsoft/winget-pkgs, copy that directory to the same path under
# manifests/, and open a pull request
```
