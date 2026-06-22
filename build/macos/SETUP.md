# One-time Mac mini setup

Reproducible recovery path if the mini ever gets rebuilt. Pairs with the
`git push origin main` flow driven by `build/macos/post-receive`.

Assumes a logged-in user `chotchki` on a recent macOS, Apple Silicon
(`aarch64-apple-darwin`). Adjust paths if the home directory differs.

## 1. Toolchain

```sh
xcode-select --install                 # full Xcode not required
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup target add aarch64-apple-darwin
```

## 2. Standard directory layout

```sh
mkdir -p "$HOME/Library/Application Support/io.hotchkiss.web/data"
mkdir -p "$HOME/Library/Logs/io.hotchkiss.web"
mkdir -p "$HOME/Library/Caches/io.hotchkiss.web"
```

## 3. Config

Place a `config.json` under `~/Library/Application Support/io.hotchkiss.web/`.
Required fields:

```json
{
  "cloudflare_token": "<secret>",
  "domain": "hotchkiss.io"
}
```

Optional `database_path`, `log_path`, `cache_path` override the defaults
that resolve under `~/Library/...` (see `src/settings.rs`).

If migrating an existing prod database: `cp -v` the
`database.sqlite`, `database.sqlite-shm`, and `database.sqlite-wal` trio
into `~/Library/Application Support/io.hotchkiss.web/data/` together —
SQLite needs all three to recover unflushed WAL state on first open.

## 4. LaunchAgent

```sh
cp build/macos/io.hotchkiss.web.plist "$HOME/Library/LaunchAgents/"
```

It will be `bootstrap`ed in step 6 once an `.app` exists in `/Applications`.

**Beta instance (Phase 12).** A second agent, `io.hotchkiss.web.beta`
(`build/macos/io.hotchkiss.web.beta.plist`), runs `Hotchkiss-IO-Beta.app`
with an explicit beta config path as `argv[1]` (the prod agent relies on the
default config location, so beta *must* point at its own config or it would
read prod's). Prerequisites before bootstrapping it: the beta log dir
`~/Library/Logs/io.hotchkiss.web.beta/` must exist, and a beta `config.json`
must be in place (see Phase 12 / 12.6 for the beta config + Cloudflare token).
Prod's label/plist are intentionally left as `io.hotchkiss.web` (not renamed).

## 5. Bare repo

```sh
mkdir -p "$HOME/repos"
git init --bare "$HOME/repos/hotchkiss-io.git"
cp build/macos/post-receive "$HOME/repos/hotchkiss-io.git/hooks/post-receive"
chmod +x "$HOME/repos/hotchkiss-io.git/hooks/post-receive"
```

On the dev machine:

```sh
git remote set-url origin ssh://<mini-host>/Users/chotchki/repos/hotchkiss-io.git
```

(or `git remote add` if `origin` is reserved for GitHub).

## 6. First deploy

The first push has nothing in `/Applications` yet; the post-receive hook
handles that case (no `mv` to `.prev` if the destination is missing).
After the push completes:

```sh
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/io.hotchkiss.web.plist"
```

Subsequent pushes are atomic-ish: rename current → `.prev`, rename new
into place, `launchctl kickstart -k gui/<uid>/io.hotchkiss.web`,
drop `.prev`. The mmap'd binary keeps the old process alive across the
rename until kickstart swaps it.

## 7. Verify

```sh
curl -sI https://hotchkiss.io/                          # 307 → /pages/Resume
launchctl print "gui/$(id -u)/io.hotchkiss.web" | head  # state running, pid set
```

## 8. Beta instance (Phase 12)

Beta runs the *same binary* as a second LaunchAgent on the same mini, on alternate
ports, with its own data dir. `git push origin main` deploys beta (bleeding edge);
prod only moves on a `v*` tag. Beta's WebAuthn `rp_id` is `hotchkiss.io` (the
registrable parent) so your existing prod passkey authenticates against beta.

1. **Directories:**

   ```sh
   mkdir -p "$HOME/Library/Application Support/io.hotchkiss.web.beta/data"
   mkdir -p "$HOME/Library/Logs/io.hotchkiss.web.beta"
   mkdir -p "$HOME/Library/Caches/io.hotchkiss.web.beta"
   ```

2. **Config** — copy the template and fill in the Cloudflare token (the **same
   one prod uses** — CF can't scope a token narrower than the `hotchkiss.io` zone):

   ```sh
   cp build/macos/beta-config.sample.json \
      "$HOME/Library/Application Support/io.hotchkiss.web.beta/config.json"
   # then edit: cloudflare_token
   ```

   No `static_ip` — beta is public, so (like prod) it discovers the public IP
   itself and its `DnsProviderService` keeps `beta.hotchkiss.io` pointed at it.
   Ports `8080`/`8443` coexist with prod's `80`/`443`. Beta deploys as a
   **release** build, so it orders a real, publicly-trusted LE-prod cert (no
   iPhone profile needed to install the PWA).

3. **LaunchAgent:**

   ```sh
   cp build/macos/io.hotchkiss.web.beta.plist "$HOME/Library/LaunchAgents/"
   ```

4. **Cloudflare (12.7) + router:** beta reuses the **prod** CF token (same
   `hotchkiss.io`-zone DNS-edit access — CF can't scope narrower). Create a
   `beta.hotchkiss.io` A record **grey-cloud
   (DNS-only)** — beta's `DnsProviderService` then keeps it pointed at the
   public IP (same as prod's `hotchkiss.io`), so beta serves its own LE cert
   end-to-end (grey, not orange/proxied). Forward external `:8443` → the mini's
   `:8443` on the router; prod keeps `:443`.

5. **Cut over the deploy hook (12.8):** re-copy `build/macos/post-receive` into
   `~/repos/hotchkiss-io.git/hooks/post-receive` (then `chmod +x`). After this,
   `git push origin main` → beta and prod only deploys on a `v*` tag — so first
   cut a bootstrap tag from current main:
   `git tag v0.x.y && git push origin v0.x.y`.

6. **Bootstrap the agent** once a `Hotchkiss-IO-Beta.app` exists in `/Applications`
   (i.e. after the first `main` push post-cutover):

   ```sh
   launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/io.hotchkiss.web.beta.plist"
   ```

The first beta deploy orders `beta.hotchkiss.io` from LE prod once; every later
`main` push snapshots prod's DB into beta (`post-receive` → `snapshot_prod_db_into_beta`)
and preserves that cert, so beta never re-orders and never trips the 5/week
duplicate-cert rate limit.
