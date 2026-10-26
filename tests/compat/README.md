# Mixed-version compatibility

`mixed_versions.py --old <velody 5.x> --new <velody 6.x>` checks a 5.x client
against a 6.0 peer and the reverse. CI runs it in the `old-release` job, with
the old build and the current build. The refusal of a v4 working repository
and the bundle round trip are covered separately by
`tests/corpus/old_release_check.py`.

Every HTTP cell starts `serve-http` on a free localhost port with
`VELODY_SERVE_TOKEN` (anonymous push is refused by default), always kills it in a
`finally`, and has the client clone, save, push, pull and fetch. Afterwards the
server's history ids must equal the client's and `fsck` must be green on both.

A path remote is not a subprocess: the client opens the remote database
in-process, so the remote's format version matters to the client binary.

| Client -> remote | HTTP (`serve-http`) | Path remote |
| --- | --- | --- |
| old (5.x) -> new (6.0, format v4) | Works. The wire protocol is unchanged. | Refused by design: the 5.x binary meets the v4 schema and exits non-zero with a version error. The remote is left unchanged. |
| new (6.0) -> old (5.x, format v3) | Works. The wire protocol is unchanged. | Works. The new client migrates the v3 remote in place to v4, so the 5.x binary can no longer open it afterwards. |

Use HTTP or ssh to serve a repository to clients you cannot upgrade.
