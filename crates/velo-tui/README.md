# velo-tui

The interactive conflict resolver for
[Velo](https://github.com/LucasVascovici/velo).

Conflicts are worked through one hunk at a time, and the file on disk stays valid
the whole way — there are no `<<<<<<<` markers pasted into your source. Each hunk
shows both sides and records a decision; `velo-merge` rebuilds the file from those
decisions once every region has one.

This crate is the presentation layer for that flow, and is published so the
workspace resolves from one place. Most people want `velo-cli`, which drives it.

MIT licensed.
