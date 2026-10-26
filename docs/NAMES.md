# Names velo publishes under

Checked on 2026-10-22 against the public registries named in each row.
Availability changes; re-check a row before relying on it. A name is
**free** when the registry answered 404 for it, **taken** when it
answered 200, and **unverified** when it could not be checked without a
login or a manual look. Unverified rows give the place to check.

## Availability

### Python and npm

| Name | Registry | Status | Checked at |
| --- | --- | --- | --- |
| `velo` (current PyPI distribution) | PyPI | **taken**: an unrelated "Velobstacles RESTful webapp" 0.0.1 | https://pypi.org/pypi/velo/json |
| `velo-core` | PyPI | free | https://pypi.org/pypi/velo-core/json |
| `velo-timeline` | PyPI | free | https://pypi.org/pypi/velo-timeline/json |
| `velo` | npm | **taken**: an unrelated 2D canvas library | https://registry.npmjs.org/velo |
| `velo-core` (unscoped) | npm | free | https://registry.npmjs.org/velo-core |
| `velo-timeline` (unscoped) | npm | free | https://registry.npmjs.org/velo-timeline |
| `@velo/core` (current npm package) | npm | package name not found (404), which does not prove the scope is free | https://registry.npmjs.org/@velo/core |
| `@velo` scope | npm | **owner's to check**: needs a login; see https://www.npmjs.com/org/velo | https://www.npmjs.com/org/velo |

### crates.io

| Crate | Published | Status | Checked at |
| --- | --- | --- | --- |
| `velo` | no | **taken**: an unrelated distributed-systems runtime, 0.20.0 | https://crates.io/api/v1/crates/velo |
| `velo-core` | yes | free | https://crates.io/api/v1/crates/velo-core |
| `velo-merge` | yes | free | https://crates.io/api/v1/crates/velo-merge |
| `velo-cli` (builds the `velo` binary) | yes | free | https://crates.io/api/v1/crates/velo-cli |
| `velo-tui` | yes | free | https://crates.io/api/v1/crates/velo-tui |
| `velo-testkit` | yes | free | https://crates.io/api/v1/crates/velo-testkit |
| `velo-ffi` | no (`publish = false`) | free | https://crates.io/api/v1/crates/velo-ffi |
| `velo-mcp` | no (`publish = false`) | free | https://crates.io/api/v1/crates/velo-mcp |
| `velo-py` | no (`publish = false`) | free | https://crates.io/api/v1/crates/velo-py |
| `velo-node` | no (`publish = false`) | free | https://crates.io/api/v1/crates/velo-node |
| `velo-wasm` | no (`publish = false`) | free | https://crates.io/api/v1/crates/velo-wasm |
| `velo-timeline` | no | free | https://crates.io/api/v1/crates/velo-timeline |

### Hosting, packaging and domains

| Name | Where | Status | Checked at |
| --- | --- | --- | --- |
| `LucasVascovici` | GitHub user | owned by the project (current home) | https://api.github.com/users/lucasvascovici |
| `velo` | GitHub organisation | **taken**: a personal account | https://api.github.com/users/velo |
| `velo` | Homebrew core formula | free | https://formulae.brew.sh/api/formula/velo.json |
| `velo` | scoop Main bucket | free | https://raw.githubusercontent.com/ScoopInstaller/Main/master/bucket/velo.json |
| `velo` | scoop Extras bucket | free | https://raw.githubusercontent.com/ScoopInstaller/Extras/master/bucket/velo.json |
| `LucasVascovici.Velo` | winget id | unverified: no manifests under the publisher path; confirm at https://github.com/microsoft/winget-pkgs/tree/master/manifests/l | https://github.com/microsoft/winget-pkgs/tree/master/manifests/l |
| `lucasvascovici/velo` | Docker Hub | free (404) | https://hub.docker.com/v2/repositories/lucasvascovici/velo |
| `ghcr.io/lucasvascovici/velo` | GitHub container registry | unverified: needs a login to list packages; see https://github.com/LucasVascovici?tab=packages | https://github.com/LucasVascovici?tab=packages |
| `velo.dev` | domain | **taken** (DNS resolves) | `nslookup velo.dev` |
| `velo.io` | domain | **taken** (DNS resolves) | `nslookup velo.io` |
| a velo-specific domain such as `velo-timeline.dev` | domain | unverified: the owner picks one and checks a registrar | the owner's registrar |

### Rows that are the owner's

| Row | Why |
| --- | --- |
| `@velo` npm scope | Cannot be checked without an npm login. |
| Trademark and prior-use search for "velo" | A legal check, not a registry lookup. |
| Whether the project keeps the name `velo` | A project decision; this page records the facts and the cost of each answer, it does not make it. |

## Recommendation for the distribution names

Keep `import velo` and the `velo` command so nobody's code changes. Only
the names of the published packages move.

- **PyPI distribution: `velo-core`.** Free, and it matches the crates.io
  crate. The import name stays `velo`.
- **npm package: `velo-core`, unscoped.** Free, and it needs no scope, so
  it does not depend on the owner's scope check. If `@velo` turns out to
  be the owner's, `@velo/core` remains an option.
- **Fallback: `velo-timeline`** on whichever registry `velo-core` has been
  taken on by the time of publishing. It is free on PyPI, npm and
  crates.io today.
- **CLI binary: `velo`.** The crate that builds it is `velo-cli`, which is
  free, so the binary name does not depend on the taken crate `velo`.

## Cost of each option

| Option | Cost |
| --- | --- |
| Keep PyPI `velo` | Not possible: the name belongs to an unrelated project. |
| PyPI `velo-core`, import `velo` (recommended) | Install line becomes `pip install velo-core`. The wheel filename becomes `velo_core-<version>-cp39-abi3-*.whl`. If someone installs both `velo-core` and the unrelated PyPI `velo`, both provide `import velo` and one overwrites the other's files; the README should say not to install both. |
| PyPI `velo-timeline`, import `velo` | The same two costs, with a name that describes the product better and no match to the crate name. |
| npm `@velo/core` (current) | Depends on the owner holding the `@velo` scope, which is not verified. |
| npm `velo-core` (recommended) | `require("velo-core")` replaces `require("@velo/core")` for the few existing users; no scope is needed. |
| Rename the project | The largest cost: the binary, every crate name, the import name, the bundle and docs text, the repository and the install channels all change, and every user's scripts break. Nothing in the tables above forces it, because free distribution names exist. |

## Decision: Velody (2026-10-26)

The owner chose the name **Velody** and a full rename (7.0.0). Checked the same day, with the URL or query
used for each:

| Name | Result |
| --- | --- |
| PyPI `velody`, `velody-core` | free (`pypi.org/pypi/<name>/json` is 404) |
| npm `velody`, `velody-core` | free (`registry.npmjs.org/<name>` is 404) |
| crates.io `velody`, `velody-core`, `velody-cli` | free (`crates.io/api/v1/crates/<name>` is 404) |
| Homebrew formula `velody` | free |
| npm scope `@velody` | **taken** (a published design-system package, `@velody/velys`) |
| GitHub user `Velody` | **taken** (an empty personal account created 2018); the repository stays under `LucasVascovici` |
| Docker Hub namespace `velody` | **taken** (no public repositories) |
| `velody.com`, `velody.io`, `velody.org` | resolve (registered); `velody.dev` and `velody.app` did not resolve |

The names that matter for publishing (the three registries) are free; the rest is worked around: unscoped
npm names, images published under another namespace, no domain yet. A trademark and prior-use search has not
been done and is the owner's. The rename itself is described in the 7.0.0 changelog.
