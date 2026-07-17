# Security policy

## Supported versions

Security fixes are made on the latest `1.0.x` release. Users should reproduce
and report a security issue on the newest published UmaDev version whenever it
is safe to do so.

## Reporting a vulnerability

Please do not open a public issue for an unpatched vulnerability, credential
exposure, or a reproducible permission-boundary bypass.

Use GitHub's private **Security → Report a vulnerability** flow for
`umacloud/umadev`. If that flow is unavailable, email the maintainer address in
the workspace package metadata (`11964948@qq.com`) with the subject
`[UmaDev security]`.

Include only the minimum evidence needed to reproduce the issue:

- UmaDev version, install method, OS, terminal, and selected base;
- the exact trust/sandbox mode and whether `--yes` was used;
- a minimal throwaway repository or redacted steps;
- the observed impact and any known workaround.

Never send API keys, login cookies, private source, or an unredacted
`.umadev/audit/` directory. We will coordinate disclosure after a fix is
available; please avoid publishing exploit details beforehand.

## Security boundaries worth reporting

- a `plan` or read-only critic session mutating the workspace;
- a base receiving broader native permissions than the selected UmaDev mode;
- a destructive, publish, deploy, push, credential-exfiltration, or
  workspace-escape action bypassing the reversibility floor;
- release/update artifacts accepted without their required integrity check;
- secrets or raw private prompts copied into global learned memory, logs, or a
  proof pack;
- command, path, archive, terminal-sequence, or MCP input injection that crosses
  its documented boundary.

## Release authenticity

Tag releases fail before artifact construction unless all publishing and native
signing credentials are configured. macOS executables are signed with a
Developer ID Application certificate, hardened runtime, and a secure timestamp,
then submitted with `notarytool` and assessed by Gatekeeper. Windows executables
are Authenticode-signed, RFC 3161 timestamped with SHA-256, and verified with
SignTool before checksums, npm packages, or GitHub attestations are created.

The release workflow expects these repository or protected-environment secrets:

- `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`;
- `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_SPECIFIC_PASSWORD`;
- `WINDOWS_CERTIFICATE_PFX_BASE64`, `WINDOWS_CERTIFICATE_PASSWORD`;
- `NPM_TOKEN`.

Certificates and passwords are decoded only on their native ephemeral runner and
are deleted before the job ends. A manual non-tag workflow run may build unsigned
test artifacts, but only a `v*` tag can publish a release and every such tag must
pass both native signature gates.
