# Release signing and auto-update

Tagging `vX.Y.Z` runs two workflows:

- `.github/workflows/release.yml` builds the `ourshell` CLI and `ourshell-mcp` for Linux, Windows and macOS. It attaches the archives with SHA-256 checksums and a GitHub build-provenance attestation.
- `.github/workflows/desktop-release.yml` builds the desktop installers (NSIS/MSI, universal DMG, deb/AppImage). It signs them, writes the updater manifest `latest.json`, and creates a **draft** release for you to review and publish.

Verify a downloaded artifact with `gh attestation verify <file> --repo NakharinA/OurShell`.

## One-time setup

### 1. Updater key (required for auto-update)

```sh
cd apps/desktop
npx tauri signer generate -w ~/.tauri/ourshell.key
```

- Put the **public** key into `apps/desktop/src-tauri/tauri.updater.conf.json` → `plugins.updater.pubkey` and commit it.
- Add repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the file contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- Back up the private key offline. If it is lost, existing installs can no longer be auto-updated.

Only release builds include the updater: the `updater` cargo feature plus `--config src-tauri/tauri.updater.conf.json`. Local and CI debug builds have no updater and need no keys. Updates are verified against the public key before installing, so a compromised download host cannot push code.

### 2. Windows Authenticode (optional, recommended)

Add `WINDOWS_CERTIFICATE` (base64 of the `.pfx`) and `WINDOWS_CERTIFICATE_PASSWORD`. The workflow imports the certificate and sets `bundle.windows.certificateThumbprint`. Without these secrets, installers are unsigned and SmartScreen will warn.

### 3. Apple codesign + notarization (optional, needed for Gatekeeper)

Add `APPLE_CERTIFICATE` (base64 `.p12` of a Developer ID Application certificate), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`.

## Cutting a release

1. Bump `version` in `Cargo.toml` (workspace), `apps/desktop/package.json` and `apps/desktop/src-tauri/tauri.conf.json`.
2. `git tag -a vX.Y.Z -m "OurShell vX.Y.Z" && git push origin vX.Y.Z`.
3. Review the draft release, then publish it. Clients see the update once `latest.json` is public.
