# setup-velo

Installs a velo release on a GitHub Actions runner and puts `velo` on `PATH`.
No package manager is needed.

```yaml
steps:
  - uses: LucasVascovici/velo/setup-velo@v6.1.0
    with:
      version: 6.1.0
  - run: velo --version
```

## Inputs

| Name | Default | Meaning |
| --- | --- | --- |
| `version` | `latest` | Release to install: `latest`, `6.1.0` or `v6.1.0`. |
| `token` | `${{ github.token }}` | Token for the GitHub API, to avoid rate limits. |

## Outputs

| Name | Meaning |
| --- | --- |
| `version` | The version reported by `velo --version`. |

## Platforms

Linux x64 and arm64, macOS x64 and arm64, Windows x64. Other runners fail with
a clear error.

## Checksums

When the release carries a `SHA256SUMS.txt` asset, the archive is verified
against it and a mismatch fails the step. Releases without one install with a
notice that verification was skipped.
