# Collaboration

### Collaboration

Velo repositories can be shared over a filesystem path (including a network or
shared drive), over SSH, or as a single self-contained file — no server to run.

#### Clone, push, pull

```bash
# Clone from a path or over SSH
velo clone /shared/project
velo clone ssh://user@host/srv/project        # ssh://[user@]host[:port]/path

# Everyday loop
velo save "Add login page"
velo push                  # fast-forward only
velo pull                  # fast-forward, or tells you it diverged
velo sync                  # fetch, merge if clean, push

# Remotes
velo remote add origin /shared/project
velo remote                             # list
velo remote remove origin

# Download without touching your branches or working tree
velo fetch
```

An HTTP remote that needs a bearer token reads it from the environment:
`VELO_TOKEN_<REMOTE>` (the remote's name, uppercased, with every character that
is not a letter or digit written as `_`; `VELO_TOKEN_ORIGIN` for `origin`), then
the general `VELO_TOKEN`. `velo clone` uses the `origin` variable. A URL such as
`https://user:secret@host/` is refused, because a credential in a URL lands in
shell history and in `velo remote`'s output. Tokens are never stored in the
repository database either, since it travels in backups. A refused request
shows as "refused access".

A path remote is opened by the client itself, not by a server, so mixed
versions behave differently there than over HTTP or ssh. A 5.x client is
refused by design when the remote is a format v4 repository (made by 6.0),
and the remote is left unchanged. A 6.0 client opens a v3 remote and migrates
it in place to v4, after which 5.x can no longer open it. To serve clients you
cannot upgrade, use `velo serve-http` or ssh, whose wire protocol is
unchanged between 5.x and 6.0.

`velo status` tells you where you stand relative to the last-fetched remote state
(no network access needed):

```
Branch: main  Position: 45877e2a3b0b3fa1  "Add login page"
  ↑ 1 ahead of origin/main (as of last fetch) — velo push to publish
```
```
  ↓ 1 behind origin/main (as of last fetch) — velo sync to catch up
  ↕ diverged from origin/main (1 ahead, 1 behind, as of last fetch) — reconcile with:
      velo sync   (or by hand: velo fetch, velo merge origin/main, velo save "Merge origin/main", velo push)
  ✔ up to date with origin/main (as of last fetch)
```

#### When two people diverge

```bash
velo push
# → Push rejected — 'main' has commits you don't have (non-fast-forward).
#   velo sync   (or by hand: velo fetch, velo merge origin/main, velo save "Merge origin/main", velo push)   (exit code 4)

velo sync
# fetches, merges what merges cleanly, saves the merge and pushes it.
# On a conflict it stops with exit code 3 and the merge in progress, nothing pushed:
velo resolve src/app.rs --take ours     # or edit it and `velo resolve --mark`
velo save "Merge origin/main"
velo sync                  # now a fast-forward push
```

`velo sync` never pushes a merge with conflicts and has no `--force`. If the
remote moves again while it runs, it exits 4 and you run it again. The manual
route (`velo fetch`, `velo merge origin/main`, `velo save`, `velo push`) still works.

Nothing is ever force-overwritten, and `pull` never rewrites your history behind
your back.

#### Offline transfer with bundles

A bundle is one self-contained file carrying snapshots, all the objects they
reference, and their tags. Useful for air-gapped machines, backups, or emailing a
branch to someone.

```bash
velo bundle create backup.velo              # whole repository
velo bundle create feature.velo feature     # everything reachable from a ref
velo bundle apply backup.velo               # import into another repository
```

Applying a bundle is **idempotent** — re-applying imports nothing and reports
that you're already up to date.

#### Only what's missing goes over the wire

Both `push` and `fetch` negotiate: the peer's known snapshots are subtracted, and
objects the peer already holds are skipped. A one-line change in a 20-file project
transfers a single object, not the whole tree — a snapshot references its entire
file tree, so naive syncing would resend everything on every push. (`bundle create`
deliberately opts out of this, since a bundle must stand alone.)

#### Hosting a remote

`velo serve-http` is a small reference server. It is safe to host once you pick
a policy; from 5.2.0 it no longer accepts anonymous pushes by default. A tested
compose deployment (Caddy proxy, tokens, read-only mirror) is in
[`examples/serve-http/`](../examples/serve-http/).

```bash
velo serve-http /srv/project                       # anonymous fetch, push refused (401)
VELO_SERVE_TOKEN=s3cret velo serve-http /srv/project   # every request needs the token
velo serve-http /srv/project --token-file tokens   # several tokens, read or write
velo serve-http /srv/project --allow-anonymous-push    # the old open behaviour
```

**Tokens.** `VELO_SERVE_TOKEN` is one write token. `--token-file FILE` holds
lines of `read <token>` or `write <token>` (`#` starts a comment; on Unix velo
warns if the file is readable by others). Once any token exists, every request
needs a valid one; a read token may fetch, only a write token may push. Clients
send theirs in `VELO_TOKEN`, or `VELO_TOKEN_<REMOTE>` (the remote's name,
uppercased) for one remote, never in the URL:

```bash
VELO_TOKEN=s3cret velo push
```

A refused request exits with an access-denied error naming the remote.

**A read-only public mirror.** `velo serve-http /srv/project --read-only`
serves anonymous fetches and refuses every push, even one carrying a valid token.

**Limits.** Each is a typed refusal on the wire (`velo: <kind>: <detail>`):

| Flag | Default | Refusal |
|---|---|---|
| `--max-request-bytes N` | 1 GiB | 413 `too-large` (clients report a limit exceeded) |
| `--request-timeout SECS` | 300 | 408 `timeout`, while receiving a body |
| `--max-connections N` | 8 | 503 `busy` when all workers are in use |

The timeout bounds a body that arrives slowly. A socket that stalls completely
is only bounded by your proxy's timeout, because the server has no socket read
timeout of its own.

**Many repositories.** `velo serve-http /srv/repos --directory` serves every
repository under `/srv/repos`, addressed as `http://host/<repo>/` (for example
`velo clone http://host/team/app/`). Paths containing `..`, percent-encoding or
backslashes are refused.

**TLS terminates at a proxy.** The server speaks plain HTTP on purpose. Run it
on `127.0.0.1` and let a proxy hold the certificate.

Caddy:

```
velo.example.com {
    reverse_proxy 127.0.0.1:8417
}
```

nginx:

```nginx
server {
    listen 443 ssl;
    server_name velo.example.com;
    # ssl_certificate / ssl_certificate_key as usual

    client_max_body_size 1g;     # match --max-request-bytes
    proxy_read_timeout 300s;     # match --request-timeout

    location / {
        proxy_pass http://127.0.0.1:8417;
    }
}
```

**The proxy's body limit is required, not optional.** When it drops a request,
tiny_http (the HTTP library under `serve-http`) first reads and discards the
unread rest of the body, allocating a buffer the size of the declared
`Content-Length`, even after velo has already answered 413. An absurd length is
caught per request, but a merely huge one can make the allocator abort the
whole server, and the library offers no way to answer without that drain. Set
`client_max_body_size` (nginx) or a `request_body { max_size ... }` block
(Caddy) so such a request never reaches velo; the proxy's read timeout also
bounds stalled clients.

**Fast-forward only.** A push the server cannot fast-forward is rejected as
non-fast-forward. There is no `--force`, on the client or the server.
