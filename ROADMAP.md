# Forge Public Roadmap

This roadmap outlines what has been shipped, what is actively in development, and what is planned for future releases of Forge.

Forge is built in public. If you have ideas, feedback, or would like to request a feature, please [open an issue](https://github.com/DiegoAndres717/forge/issues) or join the discussions.

---

## 🟢 Shipped

### v0.10.0 (Latest Release)
- **Zero-token commands & MCP Prompts**: `/forge-plans`, `/forge-memory`, `/forge-remember`, `/forge-guard` available natively across Claude Code, OpenCode, Gemini CLI, Qwen Code, and Codex.
- **Subagent Cost Delegation**: Automatic delegation advice to use cheaper models (`forge:explorer` on Haiku, `forge:reviewer` on Sonnet) for code searches, reading, and test verification.
- **Multi-Account Switching**: Manage multiple Claude Code and Codex accounts side by side with live token and quota tracking.
- **Cross-Platform Previews**: Native installer scripts and runtime support for macOS (universal), Linux, and Windows.
- **Project Guard Hooks**: Deterministic pre-commit and pre-push hooks running in isolated git worktrees.
- **Forge Memory**: Persistent cross-session and cross-agent project memory over MCP with semantic deduplication.
- **Phased Plans & Backlog**: Structured project roadmaps stored locally outside the git repository.

---

## 🟡 In Progress (Near Term)

- **Platform Parity & Official Signing**:
  - Apple Developer ID code signing & notarization for macOS (eliminating the initial Gatekeeper warning).
  - Windows Authenticode signing and winget package distribution.
  - Native Linux packages (.deb, .rpm, AUR).
- **Project Guard Visual Diff Viewer**:
  - In-app interactive diff viewer for pre-commit inspections directly inside the Guard panel.
  - Inline exceptions and file filtering before committing or creating pull requests.
- **Workspace Tabs & Multi-Window Support**:
  - Drag-and-drop terminal splits between windows.
  - Multi-monitor workspace management.
- **Extended Agent Subagent Integration**:
  - First-class subagent orchestration for Antigravity, Pi, and additional local LLM runners.

---

## 🔮 Planned (Medium & Long Term)

- **Remote & Container Workspaces**:
  - Connect Forge workspaces to remote servers via SSH or Docker containers while keeping local native GPU rendering.
- **Encrypted Team Sync for Forge Memory**:
  - Optional, end-to-end encrypted synchronization for project conventions and memory across team members (self-hosted or P2P).
- **Custom Guard Rules & Plugin Engine**:
  - WebAssembly or scriptable Guard plugins allowing teams to define custom pre-commit compliance policies.
- **Live Performance & Telemetry Dashboard**:
  - Visual metrics for agent token consumption, cache hit ratios, and estimated cost savings over time.

---

## 🤝 Contributing

Looking to contribute? Check out our issues labeled [**good first issue**](https://github.com/DiegoAndres717/forge/labels/good%20first%20issue) to get started!
