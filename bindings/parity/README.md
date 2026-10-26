# Binding parity check

`python bindings/parity/check.py` (Python 3.11+, stdlib only) fails when a
public `velody-core` embedder entry point is neither bound in the Python and
Node bindings nor listed in `exemptions.toml` with a reason. It turns "the
binding forgot a method" into a failing CI check (job `parity`).

## What counts as an entry point

Found by parsing the sources, never listed by hand:

- every module-level `pub fn` in `crates/velody-core/src/commands/*.rs`
  (named `commands::<module>::<fn>`; `mod.rs` functions are
  `commands::<fn>`), except modules declared `pub(crate)`;
- every `pub fn` in `impl Repo` / `impl WriteGuard` blocks of `repo.rs`
  and `tree.rs` (`Repo::<fn>`, `WriteGuard::<fn>`);
- module-level `pub fn`s of the modules in `EXTRA_MODULES` in `check.py`.

`#[doc(hidden)]` and `#[cfg(test)]` items are skipped.

## Bound

An entry is bound in a binding when its source under
`bindings/<name>/src/` calls it: `m::f(` for `commands::m::f`, and
`.f(`, `Repo::f(` or `WriteGuard::f(` for methods. When detection fails,
add a `[[bound]]` table with `entry`, `binding` and an `evidence` regex.

## Exempting

Add to `exemptions.toml`:

```toml
[[exempt]]
entry = "commands::save::run"
bindings = ["python", "node"]
reason = "working-tree command; the bindings expose the store-only half"
```

The reason must be concrete. For capabilities a later task will bind, use
`reason = "pending: <capability>"`; the task that binds it removes the
exemption.

## Why a stale exemption fails

An exemption for an entry that no longer exists, or that is now bound in
that binding, fails the check. Otherwise the list only grows and stops
meaning anything: binding a method must delete its exemption.
