# Good First Issues for Forge

These issues are designed for new contributors who want to start contributing to Forge. Each issue has a clear scope, pointers to the relevant code, and guidance on how to verify the change.

---

### Issue 1: `docs: add installation instructions for Arch Linux (AUR)`
- **Labels:** `good first issue`, `documentation`, `linux`
- **Difficulty:** Easy
- **Description:**
  Forge currently documents installation on macOS, Linux (`curl | sh`), and Windows. Add documentation and guidance for Arch Linux users installing via AUR or manual PKGBUILD, updating both `README.md` and `README.es.md`.
- **Where to look:**
  - `README.md` (under `#linux-preview`)
  - `README.es.md` (under `#linux-vista-previa`)
  - `scripts/install.sh`

---

### Issue 2: `feat(ui): add keyboard shortcut tooltip to terminal tab close button`
- **Labels:** `good first issue`, `enhancement`, `ui`
- **Difficulty:** Easy
- **Description:**
  In the terminal tab strip, hovering over the close (`×`) button should display a tooltip explaining that closing the tab terminates the running shell session or suggest `⌘W` / shortcut.
- **Where to look:**
  - `src/app/terminals.rs`
  - `src/theme.rs`

---

### Issue 3: `feat(guard): add predefined rule for detecting .env.local and private key files`
- **Labels:** `good first issue`, `guard`, `security`
- **Difficulty:** Easy / Medium
- **Description:**
  Project Guard checks for forbidden files and secrets before commit. Add predefined patterns for `.env.local`, `.env.*.local`, and `*.pem` / `*.key` files to the default rules template when `.forge/rules.toml` is generated.
- **Where to look:**
  - `crates/forge-core/src/guard/`
  - `crates/forge-core/src/rules.rs`

---

### Issue 4: `feat(theme): add Catppuccin Mocha theme preset`
- **Labels:** `good first issue`, `ui`, `theme`
- **Difficulty:** Easy
- **Description:**
  Forge includes light and dark native themes. Add a popular Community preset like Catppuccin Mocha to the terminal color palette options in Settings.
- **Where to look:**
  - `src/theme.rs`
  - `src/app/settings.rs`

---

### Issue 5: `feat(cli): display OS architecture and config paths in 'forge doctor'`
- **Labels:** `good first issue`, `cli`, `developer-experience`
- **Difficulty:** Easy
- **Description:**
  Enhance `forge doctor` to print the active operating system architecture (`x86_64` vs `aarch64`), config file path, and database location for easier diagnostic reporting when users open GitHub issues.
- **Where to look:**
  - `src/cli.rs`
  - `crates/forge-core/src/config.rs`
