# Roadmap

What velo has shipped, what is next, and what it has decided not to do, with
the reason. Promises about stability are in [STABILITY.md](STABILITY.md).

## Shipped

- **5.x.** The versioned-folder core: snapshots, branches, tags, merge and
  rebase with structured merge drivers, bundles, sync over local, HTTP and
  stream remotes, signed history, retention and compaction, `fsck`, and the
  embedder API with its bindings.
- **6.0.** Repository format v4, with a one-way migration from v3 and a bundle
  as the way back. See [FORMAT.md](FORMAT.md).
- **6.1.** Fixes from the team simulation, tags published by push, a
  merge-in-progress record that survives save and abort, and timing laps for
  clone. 6.1.0 is the latest release.

## Next

Work for 6.2, in progress:

- Faster saves.
- Faster blame.
- Distribution naming for the Python and npm packages (`velo-core`).

## Declined or deferred

Gaps that are decisions rather than defects. Each is accepted, declined or
deferred, with the reason.

| Topic | Decision | Reason |
|---|---|---|
| Cross-process change notification | Accepted as documentation | Events are in-process only. A poll on `Repo::head_token` works across processes today; a file-based notify is built only if an adopter asks. |
| A managed or hosted option | Declined for now | `serve-http` has bearer tokens and a deployment example, but no tenants, quotas or UI. Revisit when a hosted consumer exists. |
| Continuous git mirroring | Under consideration; a one-way mirror is the likely first step, pending user evidence | Export and import exist, but a team cannot adopt velo without leaving git. |
| More language bindings (Go, Java, .NET) | Declined until asked | They could use the C ABI, which stays stable and documented. |
| An editor or IDE view | Declined for now | `velo report` is static; its model is data an editor can render. |
| A delta wire (v5) | Deferred | The wire is full trees so mixed versions work. It will be decided on its own measurement, not before. |
| Windows first save under Defender | Accepted as a documented limit | The cost is per-file open, bound to the platform. See [WINDOWS.md](WINDOWS.md); revisit only if an adopter on Windows hits it. |

### Polling for changes from another process

`Repo::head_token(&self) -> Result<u64>` hashes snapshots, branches and tags
into one integer. Equal tokens mean nothing tracked changed; a different token
means re-read what you care about. It does not cover the working tree.

```rust
let mut last = repo.head_token()?;
loop {
    std::thread::sleep(std::time::Duration::from_millis(500));
    let now = repo.head_token()?;
    if now != last {
        last = now;
        // re-read branches, history or status here
    }
}
```
