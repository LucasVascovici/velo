# Security policy

## Supported versions

Security fixes go into the latest minor release. Older minors are not patched;
upgrade to the latest one.

## Reporting a vulnerability

Please report vulnerabilities privately, not in public issues, using GitHub
private vulnerability reporting:

<https://github.com/LucasVascovici/velo/security/advisories/new>

What to expect:

- an acknowledgement within 7 days;
- a fix or a written mitigation plan within 90 days;
- coordinated disclosure: please do not publish details until a fix is
  released or the 90 days have passed, whichever comes first. Reporters are
  credited in the release notes unless they ask not to be.

## What velo defends against, and what it does not

See [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md). Reports about something the
model lists as out of scope are welcome as hardening ideas, but are not treated
as vulnerabilities.

## Unsafe code

Unsafe is forbidden or justified in every crate. Crates that need it deny
`unsafe_code` and allow it per site, with a `SAFETY` comment on each block.

| Crate | Unsafe sites | Reason | Lint level |
|-------|--------------|--------|------------|
| velo-core | 1 | read-only `memmap2` map in `storage.rs`, copied out at once | deny, allowed at the site |
| velo-cli | 3 | `kill(pid, 0)` on Unix and `OpenProcess`/`GetExitCodeProcess` on Windows (`lockwait.rs`); `SetConsoleOutputCP` on Windows (`main.rs`) | deny, allowed at the sites |
| velo-ffi | by nature | C ABI: raw pointers, `CStr`/`CString`/`Box::from_raw`; every `unsafe fn` has a `# Safety` section | unsafe permitted, documented |
| velo-merge | 0 | none | forbid |
| velo-tui | 0 | none | forbid |
| velo-testkit | 0 | none | forbid |
| velo-mcp | 0 | none (library and binary) | forbid |
| prompt-registry | 0 | none | forbid |
| bindings/python | 0 | no unsafe block | none needed |

## Supply chain

- `cargo deny` (advisories, licences, bans, sources; see `deny.toml`) and
  `cargo audit` run as CI jobs.
- Releases carry an SBOM, and a CI job checks that builds are reproducible.
- Every reader of untrusted bytes is fuzzed in CI (`fuzz/`).
