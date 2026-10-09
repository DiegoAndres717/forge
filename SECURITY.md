# Security

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through
[GitHub Security Advisories](https://github.com/DiegoAndres717/forge/security/advisories/new).
You'll get an answer within a few days.

## How Forge protects you

- **Local only.** Forge has no telemetry and no account. Its only network calls are the
  update check against GitHub Releases and the optional AI review you configure.
- **Verified updates.** Updates are downloaded from GitHub Releases over HTTPS and installed
  only if their SHA-256 matches the checksum published with the release.
- **Dangerous commands** (`rm -rf` outside the project, `git push --force`, `DROP DATABASE`…)
  need your approval in the window, even when an AI agent runs them.
- **Supply chain.** `main` is protected: every change goes through a pull request with green
  CI. CI audits dependencies against the RustSec database, GitHub Actions are pinned to
  commit SHAs, and Dependabot keeps both up to date.

Forge is not yet signed with an Apple Developer ID, so macOS asks for confirmation the first
time you open it (see the README).
