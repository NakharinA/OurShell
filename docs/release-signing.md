# Release signing and auto-update

Tagging `vX.Y.Z` runs two workflows, both gated on `scripts/check-version.mjs --tag vX.Y.Z` passing first (it fails the whole workflow early if the tag doesn't match the workspace version):

- `.github/workflows/release.yml` builds the `ourshell` CLI and `ourshell-mcp` for the same five targets as the desktop app (Windows x64/ARM64, macOS ARM64, Linux x64/ARM64). It attaches the archives (`ourshell-<tag>-<target>.zip|tar.gz`) with SHA-256 checksums (`SHA256SUMS-<target>.txt`) and a GitHub build-provenance attestation. These are exactly the asset names `ourshell update` looks for.
- `.github/workflows/desktop-release.yml` builds the desktop installers for Windows x64 (NSIS/MSI), Windows ARM64 (NSIS), macOS ARM64 (DMG), Linux x64 and Linux ARM64 (deb/rpm/AppImage). It signs them, writes the updater manifest `latest.json` (merged across all five platforms), and creates a **draft** release for you to review and publish.

Both workflows publish into the **same** draft release (same tag, same release name, `draft: true`), so one release holds the CLI/MCP archives and every desktop installer.

Verify a downloaded artifact with `gh attestation verify <file> --repo NakharinA/OurShell`.

## One-time setup

### 1. Updater key (required for auto-update)

```sh
cd apps/desktop
npx tauri signer generate -w ~/.tauri/ourshell-updater.key
```

- Put the **public** key into `apps/desktop/src-tauri/tauri.updater.conf.json` → `plugins.updater.pubkey` and commit it (already done for this repository).
- Add repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the file contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- Back up the private key offline. If it is lost, existing installs can no longer be auto-updated, and there's no way to re-sign in place -- recovering requires everyone to reinstall manually once with a new key.

Only release builds include the updater: the `updater` cargo feature plus `--config src-tauri/tauri.updater.conf.json`. Local and CI debug builds have no updater and need no keys. Updates are verified against the public key before installing, so a compromised download host cannot push code.

**Key custody.** The maintainer keeps the private key at `~/.tauri/ourshell-updater.key` (plus its `.password` file) on their own machine, and the same material lives in the repository's `TAURI_SIGNING_PRIVATE_KEY`/`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets. Losing both copies means existing installs can no longer verify new signed updates (they'll keep running, just never auto-update again); rotating the key (e.g. after a suspected compromise) means generating a new keypair, updating `tauri.updater.conf.json` and the secrets, and accepting that every existing install needs a one-time manual reinstall to pick up the new public key, since an update signed with the new key can't be verified by an app that only trusts the old one.

Windows builds both NSIS and MSI installers; since the updater can only point at one artifact per platform, `desktop-release.yml` passes `updaterJsonPreferNsis: true` to `tauri-action` so `latest.json` references the NSIS installer (it supports the passive/silent install the updater config asks for; MSI's UI can't be suppressed the same way).

**AppImage is the updatable Linux format.** The Tauri updater can only replace an AppImage in place; it has no mechanism for an app installed via `dpkg`/`rpm`. The same binary (built once with `--features updater`) goes into all three Linux bundles, so the in-app "Install and restart" banner still appears on a `.deb`/`.rpm` install -- but clicking it will fail there. Users who installed the `.deb` or `.rpm` should instead download the new package from the release page and reinstall with their package manager (`apt install ./ourshell_X.Y.Z_amd64.deb`, `dnf install ./ourshell-X.Y.Z.x86_64.rpm`); prefer the AppImage if you want in-app updates to work.

**Publishing is the gate.** `desktop-release.yml` always creates the release as a **draft**, even with `includeUpdaterJson: true`. `latest.json` is uploaded to the draft, but GitHub does not serve a draft release's assets at the `releases/latest/download/...` URL the updater polls -- so existing installs never see the update, and nothing changes for them, until you review and **publish** the release.

### 2. Windows Authenticode (optional, recommended)

Add `WINDOWS_CERTIFICATE` (base64 of the `.pfx`) and `WINDOWS_CERTIFICATE_PASSWORD`. The workflow imports the certificate and sets `bundle.windows.certificateThumbprint`. Without these secrets, installers are unsigned and SmartScreen will warn.

### 3. Apple codesign + notarization (optional, needed for Gatekeeper)

Add `APPLE_CERTIFICATE` (base64 `.p12` of a Developer ID Application certificate), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`.

## Cutting a release

1. `node scripts/bump-version.mjs X.Y.Z` -- rewrites the version everywhere it's recorded (workspace `Cargo.toml`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/package.json`). Run `node scripts/check-version.mjs` if you want to double-check they agree (CI does this on every push/PR too).
2. `git add -A && git commit -m "chore: bump version to X.Y.Z"`.
3. `git tag -a vX.Y.Z -m "OurShell vX.Y.Z" && git push --follow-tags`.
4. Both workflows run `scripts/check-version.mjs --tag vX.Y.Z` first and fail immediately if the tag doesn't match the version just committed, before building anything.
5. Review the draft release (CLI/MCP archives + checksums from `release.yml`, desktop installers + `latest.json` from `desktop-release.yml`), then **publish** it. Desktop clients see the update, and `ourshell update` sees the new CLI release, only once the release is public.
