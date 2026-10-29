# Contributing to OurShell

Thanks for helping! Bug reports, docs fixes and pull requests are all welcome.

## Getting started

Requirements: Rust (stable, via rustup), Node 22, and on Windows the MSVC C++
build tools. The sync server's tests use PostgreSQL (see `docs/self-hosting.md`).

```sh
cargo build --workspace
cargo test --workspace          # server DB tests skip if Postgres isn't reachable
cd apps/desktop && npm ci && npm run tauri dev
```

Crates must not pull in `aws-lc-sys` (it needs NASM on Windows); use the `ring`
backends. Check with `cargo tree -i aws-lc-sys`.

## Before you open a pull request

- `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- desktop changes: `npm run check` (0 errors, 0 warnings) and `npx vitest run`
- add tests for bug fixes and new behaviour
- update docs in `docs/` when behaviour or protocols change

Keep pull requests focused: one change per PR, with a description of *why*.

## Security-sensitive areas

Changes to crypto, the vault format, the agent's AI policy/approval flow,
redaction, or the sync protocol get extra review. Never write custom crypto
primitives; use well-known audited crates. Report vulnerabilities privately,
see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).
