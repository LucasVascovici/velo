# Demo session

A demo that is data, not a video: [`transcript.md`](transcript.md) is a
scripted terminal session, written by [`demo.sh`](demo.sh) from the real
`velo` binary. In it an agent run goes wrong, the history shows where
(`velo history`, `velo blame`, `velo show`), `velo undo` recovers, and the
session ends with `velo report`. The checked-in report it points to is
[`docs/report-sample.html`](../report-sample.html).

## Regenerate it

```console
$ cargo build --release -p velo-cli
$ docs/demo/demo.sh target/release/velo
```

The script is **Unix only**: it needs bash, sed and mktemp, and CI runs it on
Linux. It does not run on Windows outside a Unix shell such as WSL.

Output that varies between runs is replaced with fixed text before it reaches
the transcript: snapshot ids become `<snap-1>`, `<snap-2>` and so on in the
order they appear, times become `2025-01-06 09:00`, and the temporary
directory becomes `/tmp/demo`. Authors come from `VELO_AUTHOR_NAME` and
`VELO_AUTHOR_EMAIL`. Two runs in a row give the same file.

## Checked in CI

[`.github/workflows/demo.yml`](../../.github/workflows/demo.yml) rebuilds the
transcript and fails if it differs from the committed one. When a change to
velo's output makes it fail, run the script above and commit the new
`transcript.md`.
