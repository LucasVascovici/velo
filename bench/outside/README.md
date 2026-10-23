# Outside readers and consumers

Velo's own tests and the cold-start agents (`bench/coldstart/`) cannot tell
us whether a stranger understands the project. This directory holds what is
needed to ask one, and the place to record the answer.

## For the owner

Who qualifies: someone who has never seen velo, who is not Velum, and who
did not write any part of this project.

- **Outside reader**: one to three people. Give them the README and twenty
  minutes. Record `reader-notes.md` for each.
- **Outside consumer**: one person or project. Ask them to wire velo into
  something real using only the public docs. Record `consumer-notes.md`.

How to send it: paste the brief below, with the release link. Nothing else:
no walkthrough, no hints. Copy the matching notes form, fill it in while they
answer, and save it as `reader-<tag>-<n>.md` or `consumer-<tag>-<n>.md` here.
Answers are committed verbatim, never summarised. Add one row per person to
`findings.md`, and say where they disagree with the cold-start agents'
`bench/coldstart/friction-log-<tag>.md`.

## The brief (copy and paste)

```
Hi. I would like your honest reading of a project called velo.

Velo is a timeline engine: verifiable whole-tree snapshots, provenance and
explicit divergence, in one SQLite file. It is an embeddable Rust library
first, with bindings, an MCP server and a CLI on top.

Reader: read the README for twenty minutes, then tell me what it is for,
who it is not for, and what you would try first.
Consumer: wire velo into something real, using only the public docs. Take
an evening; your own judgement.

I will record your answers word for word, plus what you found missing or
broken. Results are published as findings, with your name or anonymously,
as you choose.

Use the latest release at https://github.com/LucasVascovici/velo/releases
```

## The release to use

The latest release at https://github.com/LucasVascovici/velo/releases,
installed per [docs/INSTALL.md](../../docs/INSTALL.md), never a checkout.
Write the release tag on the notes form.
