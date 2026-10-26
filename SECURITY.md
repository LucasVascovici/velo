# Security policy

## Supported versions

Security fixes go into the latest minor release. Older minors are not patched;
upgrade to the latest one.

## Reporting a vulnerability

Please report vulnerabilities privately, not in public issues, using GitHub
private vulnerability reporting:

<https://github.com/LucasVascovici/velody/security/advisories/new>

What to expect, and when: see [Disclosure and fix timeline](#disclosure-and-fix-timeline).

## Disclosure and fix timeline

Days count from the day the report is received.

- **7 days**: acknowledgement.
- **14 days**: initial assessment (accepted or not, and a severity).
- **30 days**: a fix or a written mitigation for high-severity issues. Lower
  severities are fixed on a best-effort schedule, inside the 90 days.
- **90 days at most**: coordinated disclosure. Please do not publish details
  until a fix is released or 90 days have passed, whichever comes first.
- **Credit**: reporters are credited in the release notes by default, unless
  they ask not to be.

Fixes ship as a patch release of the latest minor, per the Supported versions
section, together with a GitHub Security Advisory.

## What Velody defends against, and what it does not

See [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md). Reports about something the
model lists as out of scope are welcome as hardening ideas, but are not treated
as vulnerabilities.

## Unsafe code

Unsafe is forbidden or justified in every crate. Crates that need it deny
`unsafe_code` and allow it per site, with a `SAFETY` comment on each block.

| Crate | Unsafe sites | Reason | Lint level |
|-------|--------------|--------|------------|
| velody-core | 1 | read-only `memmap2` map in `storage.rs`, copied out at once | deny, allowed at the site |
| velody-cli | 3 | `kill(pid, 0)` on Unix and `OpenProcess`/`GetExitCodeProcess` on Windows (`lockwait.rs`); `SetConsoleOutputCP` on Windows (`main.rs`) | deny, allowed at the sites |
| velody-ffi | by nature | C ABI: raw pointers, `CStr`/`CString`/`Box::from_raw`; every `unsafe fn` has a `# Safety` section | unsafe permitted, documented |
| velody-merge | 0 | none | forbid |
| velody-tui | 0 | none | forbid |
| velody-testkit | 0 | none | forbid |
| velody-mcp | 0 | none (library and binary) | forbid |
| prompt-registry | 0 | none | forbid |
| bindings/python | 0 | no unsafe block | none needed |

## Supply chain

- `cargo deny` (advisories, licences, bans, sources; see `deny.toml`) and
  `cargo audit` run as CI jobs.
- Releases carry an SBOM, and a CI job checks that builds are reproducible.
- Every reader of untrusted bytes is fuzzed in CI (`fuzz/`).
