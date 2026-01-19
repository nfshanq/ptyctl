# ptyctl MCP PTY Key Support Plan

Goal: improve PTY key support for MCP, accept common key string formats (e.g., `ctrl+c`), ensure `ptyctl_session_io` key parsing is stable, and cover new behavior with automated tests.

## Scope
- In scope:
  - Extend `SessionKey` deserialization aliases to accept `ctrl+c` / `ctrl-c` and similar forms.
  - Add hyphenated aliases for common arrow/page keys (e.g., `arrow-up`) when needed.
  - Add unit tests covering all supported keys and new aliases.
  - Run formatting and tests to ensure everything passes.
- Out of scope:
  - Add complex chord semantics (e.g., Alt/Meta multi-key sequences).
  - Change MCP protocol or RMCP SDK structural behavior.

## Implementation steps
1. Review current `SessionKey`, `SessionIoRequest`, and `key_bytes` usage paths (`src/session/mod.rs`, `src/mcp.rs`).
2. Add `+`/`-` aliases for ctrl keys; add `-` aliases for arrow/page keys.
3. If the build script uses unstable syntax and tests cannot run, convert it to stable Rust (e.g., `build/build.rs`).
4. Add unit tests:
   - Cover all canonical values (snake_case).
   - Cover new aliases (`ctrl+c`, `ctrl-c`, `ctrl+backslash`, etc.).
   - Verify `key_bytes` mappings do not regress.
5. Run `cargo fmt`, `cargo test`, and optionally `cargo clippy --all-targets --all-features -D warnings`.

## Test plan
- Parsing tests: run `serde_json::from_str` for all `SessionKey` input strings and verify parsing succeeds.
- Behavior tests: call `key_bytes` for each `SessionKey` and assert expected sequences (cover Enter/Tab/Backspace/Delete/Home/End/arrow keys/PageUp/PageDown and ctrl keys).
- Regression tests: ensure `cargo test` passes; if CI or local lint requires, also run `cargo clippy`.
