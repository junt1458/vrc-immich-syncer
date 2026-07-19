# AGENTS.md

Guidance for AI agents (and humans) working in this repository.

## Project

`vrc-immich-syncer` — a Rust resident client that watches VRChat screenshot
folders and uploads new images to an Immich server. Primary target is Windows;
core functionality (scan / watch / upload) must keep working on Linux/macOS.
The full architecture and milestone plan lives in `PLAN.md`, which is a local
working document (git-ignored, not part of the repository).

## Repository Layout

Cargo workspace:

- `crates/immich-client` — typed async Immich REST client. No app logic here.
- `crates/syncer-core` — config, SQLite upload ledger, scanner, watcher, upload pipeline.
- `crates/syncer-app` — the binary: egui/eframe window, tray-icon, notifications.

## Build & Test

```sh
cargo fmt --all              # format (CI enforces --check)
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p syncer-app      # run the client
```

The devcontainer is Linux; GUI/tray crates require system packages:
`libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`. Windows-only behavior
(toasts, tray visuals) is validated by the Windows CI build and manual testing.

## Testing Policy

- Everything that can be verified by automated tests **must** be covered by
  unit/integration tests: API client behavior (against a mock server), config
  load/save, the SQLite ledger, scanner diffing, retry/backoff logic, the
  VRChat/VRCX PNG metadata parser (fixture files), etc.
- Do not rely on manual runs to validate testable logic; manual verification is
  reserved for what genuinely cannot be automated (tray visuals, toast
  notifications, login auto-start registration, real-server behavior) and is
  performed by the user at the end.
- Run `cargo test --workspace` (plus fmt/clippy) before every commit.

## Conventions

- Rust 2021+ edition, `rustfmt` defaults, clippy-clean (`-D warnings`).
- Keep Immich API knowledge inside `immich-client`; keep UI-free logic inside
  `syncer-core` so it stays testable without a display.
- The SQLite ledger is the source of truth for "already uploaded": files marked
  `uploaded`/`duplicate` must never be uploaded again, even if the asset was
  deleted on the server.
- Documentation and code comments are written in English.

## Commits

- Conventional Commits style: `feat(scope): …`, `fix: …`, `docs: …`, etc.
- Commit at meaningful granularity (one milestone/feature per commit, not one
  giant commit at the end).
- Commit messages are written in English.
- **Every AI-authored commit must include a co-author trailer**, e.g.:

  ```
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  ```

## Subagent Workflow

- Implementation work is delegated to subagents per the milestone table in
  PLAN.md; independent milestones run in parallel.
- Subagents work only within their assigned crate/milestone and must leave the
  workspace compiling (`cargo check --workspace`) when done.
- The coordinating agent reviews diffs, runs fmt/clippy/tests, and creates the
  commits.
