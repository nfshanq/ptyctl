# Repository Guidelines

## Project Structure

- `src/`: Rust application source. Entry point is `src/main.rs`.
- `Cargo.toml`: Crate metadata and dependencies.
- `tests/`: Integration tests (create as needed).
- `dev-notes/`: Design notes and scratch docs (not part of the build).
- `target/`: Build artifacts (gitignored).

## Build, Test, and Development Commands

Run all commands from the repo root:

- `cargo build`: Compile the project (debug profile).
- `cargo run -- [args]`: Build and run the CLI locally.
- `cargo test`: Run unit + integration tests.
- `cargo fmt`: Format code with Rustfmt.
- `cargo clippy --all-targets --all-features -D warnings`: Lint with Clippy and fail on warnings.

Example: `cargo run -- --help`

## Coding Style & Naming Conventions

- Formatting: use `cargo fmt` (Rustfmt defaults). Prefer stable, idiomatic Rust.
- Linting: keep `cargo clippy` clean; avoid `unwrap()` in non-test code unless justified.
- Naming:
  - Types: `PascalCase` (e.g., `PtySession`).
  - Functions/vars/modules: `snake_case` (e.g., `spawn_pty`).
  - Constants: `SCREAMING_SNAKE_CASE`.
- Files/modules: `snake_case.rs` aligned with module names.

## Language Requirements

- All code, comments, and git commit messages must be written in English.
- Terminal output and user-facing messages can be in Chinese when appropriate.

## MCP SDK Requirements

- Implement MCP using the RMCP SDK: https://github.com/modelcontextprotocol/rust-sdk (`rmcp` crate).
- Use RMCP stdio transport for STDIO mode and RMCP streamable HTTP server for HTTP mode.
- Generate tool parameter schemas via `schemars` and RMCP tool macros; do not hand-roll MCP JSON-RPC handling.

## Testing Guidelines

- Prefer fast unit tests near the code (`mod tests { ... }` in the same file).
- Use `tests/*.rs` for integration tests that exercise the binary end-to-end.
- Test names should describe behavior (e.g., `prints_help_for_invalid_args`).

## Commit & Pull Request Guidelines

- No established commit-message convention yet (new repo). Recommended: Conventional Commits,
  e.g., `feat: add basic CLI parsing`, `fix: handle empty input`.
- PRs should include:
  - A brief summary + rationale.
  - How to test (exact commands).
  - Any user-visible changes (CLI flags/output examples).

## Configuration & Security Notes

- Avoid committing secrets or machine-specific paths.
- Prefer configuration via CLI flags and environment variables; document new flags in the PR description.
