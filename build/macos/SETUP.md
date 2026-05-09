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
