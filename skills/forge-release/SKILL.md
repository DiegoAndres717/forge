---
name: forge-release
description: Ship a new Forge version.
---

# New version

Only when the project owner asks for it.

1. Branch `chore/release-X.Y.Z` from an up-to-date `main`.
2. Bump `version` in the root `Cargo.toml` and run `cargo build` so `Cargo.lock` updates.
3. `cargo fmt --all --check`, commit `chore(release): vX.Y.Z` and a PR summarizing what the version brings.
4. Once CI passes and it's merged: on an up-to-date `main`, `git tag vX.Y.Z && git push origin vX.Y.Z`.
5. The `release.yml` workflow checks the tag matches the version, builds the universal DMG and its `.sha256`, publishes the GitHub release and updates the Homebrew recipe in `DiegoAndres717/homebrew-tap` (with the `HOMEBREW_TAP_TOKEN` secret).
6. Check the release has `Forge-X.Y.Z.dmg` and `Forge-X.Y.Z.dmg.sha256` (`gh release view vX.Y.Z`).

Install for users: `curl -fsSL https://raw.githubusercontent.com/DiegoAndres717/forge/main/scripts/install.sh | sh` or `brew install --cask diegoandres717/tap/forge`; the app also announces updates.
