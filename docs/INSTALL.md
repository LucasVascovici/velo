## Installation

### With cargo

```bash
cargo install --git https://github.com/LucasVascovici/velody velody-cli
```

Builds from source, so it works anywhere Rust does — including platforms with no
pre-built binary. Requires Rust 1.88 or later.

The crates are packaged and ready for crates.io — `cargo install velody-cli` is
what this becomes — but nothing is published there yet.

### Python and Node bindings without a registry

Each release attaches the abi3 Python wheels, the Node tarballs and a checksum
file. Nothing is on PyPI or npm; a release asset can be removed, so this is
reversible, unlike a registry version. Replace `vX.Y.Z` with a release tag.

```bash
# Python (abi3 wheel, one per platform: velody_core-<version>-cp39-abi3-<platform>.whl)
pip install https://github.com/LucasVascovici/velody/releases/download/vX.Y.Z/<wheel file>

# Node (velody-core-<version>-<platform>.tgz, platform is linux, macos or windows)
npm install https://github.com/LucasVascovici/velody/releases/download/vX.Y.Z/velody-core-<version>-<platform>.tgz

# The CLI, built from the tag
cargo install --git https://github.com/LucasVascovici/velody --tag vX.Y.Z velody-cli
```

To verify a download, fetch `SHA256SUMS-bindings.txt` from the same release,
put it next to the files and run `sha256sum --check --ignore-missing
SHA256SUMS-bindings.txt` (`shasum -a 256 -c` on macOS).

The bindings are 0.x and unpublished; see [docs/STABILITY.md](STABILITY.md)
for what that promises.

### Unix (Linux & macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velody/main/install.sh | sh
```

By default the binary is installed to `/usr/local/bin` (with `sudo` if needed) or `~/.local/bin` if sudo is unavailable. Options:

```bash
# Install to a custom directory
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velody/main/install.sh | sh -s -- --dir ~/.local/bin

# Preview what the script would do without making any changes
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velody/main/install.sh | sh -s -- --dry-run
```

### Windows

Download the latest `velody-x86_64-windows.zip` from the [Releases page](https://github.com/LucasVascovici/velody/releases), extract `velody.exe`, and place it anywhere on your `PATH`.

Large trees save slowly the first time on Windows because of Defender scanning; see [docs/WINDOWS.md](WINDOWS.md) for measured numbers and the settings that help.

### As a library

Velody's core is a separate crate, so a program can use the repository, history
and merge engine without the CLI. It is not on crates.io yet; depend on it from
git:

```toml
[dependencies]
velody-core = { git = "https://github.com/LucasVascovici/velody", tag = "v5.3.0" }
```

It produces no terminal output at all — every command returns data — which is
what makes it usable from a GUI or a server. See
[`crates/velody-core`](../crates/velody-core) for the library's own README, and
[`examples/prompt-registry`](../examples/prompt-registry) for a worked example with
no working tree.

Optional features, both off by default so an embedder does not pay for the CLI's
dependencies: `bundle` (offline history transfer) and `ssh` (sync over a spawned
server process).

The Python, Node.js, C, WebAssembly and MCP surfaces are built from source too;
see [Use it from your language](LIBRARY.md).

### Pre-built binaries

| Platform | File |
| :--- | :--- |
| Linux x86-64 (musl, static) | `velody-x86_64-linux.tar.gz` |
| Linux ARM64 (musl, static) | `velody-aarch64-linux.tar.gz` |
| macOS Apple Silicon | `velody-aarch64-macos.tar.gz` |
| macOS Intel | `velody-x86_64-macos.tar.gz` |
| Windows x86-64 | `velody-x86_64-windows.zip` |

Each release also carries a CycloneDX software bill of materials for `velody-cli`, attached as `velody-<version>.cdx.json`. Release builds are checked for reproducibility by `.github/workflows/reproducible.yml`, which builds the Linux x86-64 musl binary twice from the same tag in different directories and fails if the hashes differ; no differences are known so far.

### Verify a download

Every release asset carries a GitHub build-provenance attestation, and the
checksum files are signed keylessly with cosign using the release workflow's
GitHub identity. No key is involved.

```bash
# Checksums: SHA256SUMS.txt covers the CLI and MCP archives and the SBOM,
# SHA256SUMS-bindings.txt covers the wheels and Node tarballs.
sha256sum -c --ignore-missing SHA256SUMS.txt   # shasum -a 256 -c on macOS, with every named file present
# PowerShell
(Get-FileHash velody-x86_64-windows.zip -Algorithm SHA256).Hash.ToLower()  # compare with SHA256SUMS.txt

# Build provenance of one file
gh attestation verify velody-x86_64-linux.tar.gz --repo LucasVascovici/velody

# Signature over the checksum file (the same works for SHA256SUMS-bindings.txt)
cosign verify-blob --bundle SHA256SUMS.txt.sigstore.json   --certificate-identity-regexp 'https://github.com/LucasVascovici/velody/.github/workflows/release.yml@refs/tags/v.*'   --certificate-oidc-issuer https://token.actions.githubusercontent.com   SHA256SUMS.txt
```

### Build from source

Requires Rust 1.88 or later.

```bash
git clone https://github.com/LucasVascovici/velody.git
cd velody
cargo build --release
# Binary is at: target/release/velody
```
