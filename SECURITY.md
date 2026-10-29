# Security policy

OurShell stores SSH keys and passwords and lets AI clients run commands on your
servers, so we take security reports seriously.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through GitHub: go to the repository's **Security** tab and click
**Report a vulnerability** (GitHub private vulnerability reporting). Include:

- what is affected (crate, file, or feature) and the version or commit,
- steps to reproduce or a proof of concept,
- the impact you expect (for example: secret disclosure, approval bypass, RCE).

We aim to acknowledge reports within 7 days and to agree on a disclosure date
with you once a fix is ready. Credit is given in the release notes unless you
prefer otherwise.

## Scope

In scope: the vault and crypto (`crates/core`), the local agent and AI policy
(`crates/cli/src/agent`), the MCP server (`crates/mcp`), the sync server
(`server/`), the desktop app (`apps/desktop`), and release/signing workflows.

Known limitations that are not considered vulnerabilities are listed in
[`docs/security-review-request.md`](docs/security-review-request.md) and the
[threat model](docs/threat-model.md).

## Supported versions

Only the latest release receives security fixes while the project is pre-1.0.
