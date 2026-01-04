# ptyctl: Rust-based MCP (STDIO/HTTP) Remote Interactive Controller — Requirements Specification

## 1. Background & Goals
We will build an MCP server written in **Rust** (hereafter `ptyctl`) so that upper-layer callers (e.g., Codex) can perform "remote interactive operations" via MCP tool calls, rather than running `ssh`/`expect` locally.

Key motivations:
- Target endpoints can be Linux hosts or network devices such as routers/switches.
- The target endpoints may **not be able to install any software**, so we must rely on existing protocols such as **SSH / Telnet**.
- Operations can be strongly interactive (e.g., `docker exec -it`, entering router `config terminal`, pagers/confirm prompts/password prompts, etc.).
- The MCP server should support both **STDIO** and **HTTP** transports.

The deliverable is not an "automation script", but a **reusable, controllable PTY session control layer**: open sessions, send input, read output, (optionally) obtain an exit code, interrupt/timeout, and manage multiple concurrent sessions.

## 2. Requirements Assessment & Analysis
### 2.1 Feasibility
Feasible. Implement a local MCP server using Rust + Tokio, and internally support:
- SSH: create connections, request a remote PTY, interactive read/write; provide a reliable `exit_code` mechanism for non-fullscreen commands (via injected end markers).
- Telnet: establish a connection and interactive read/write over the same stream; "exec + exit code" can only be **best-effort** (depends on whether the remote shell/device CLI can output a parsable marker / echo).

### 2.2 Key Risks
- **"exec + exit code" is not generally portable on Telnet**: different device CLIs may not have `$?`, may not support `printf`, and may not have a meaningful exit code concept.
- **Interactive full-screen programs are not structurally modelable**: programs like `vim/top/less` cannot be reliably captured as `stdout/stderr/exit_code`; they must be driven via streaming read/write plus caller strategy for exiting.
- **Prompt/pager/confirm prompts vary widely**: do not treat "prompt detection" as a correctness prerequisite; rely on caller-provided `until_regex` / end markers.
- **Security & compliance**: handling passwords/private keys, HostKey policy, log redaction, and Telnet cleartext risk warnings.

### 2.3 Key Assumptions
- `ptyctl` runs in a trusted local environment (developer machine / jump host) with network access.
- The caller (e.g., Codex) can use the tools as specified: non-interactive commands via `exec`, strongly interactive flows via a `write/read` loop.
- HTTP mode is mainly for local/LAN usage and is not required to be exposed to the public Internet.

## 3. Scope
### 3.1 In Scope (Must Have)
- An MCP server written in Rust: support both `stdio` and `http` runtime modes (same toolset and semantics).
- Support two protocols: `ssh` and `telnet`.
- Telnet protocol adapter: implementation requirements are defined in `telnet-spec.md` (protocol parsing, common negotiation, async streaming output, concurrency, etc.).
- Session management: open/close/list, concurrent sessions, configurable idle timeout cleanup.
- Interactive capabilities: write arbitrary bytes, read byte streams, send control keys (Ctrl-C etc.), optional PTY resize.
- Non-interactive execution (at least for SSH): `exec` returns `stdout/stderr/exit_code` and uses a stable exit-code extraction protocol (end marker).
- SSH connectivity: do not restrict authentication methods; must be compatible with OpenSSH client capabilities (including `ProxyJump` bastions, keyboard-interactive/MFA, etc.).
- Observability: structured logs and per-session basic metrics (created time, last activity, cumulative read/write bytes, error cause, etc.).

### 3.2 Out of Scope (Not in the first release; may be iterated later)
- Installing an agent on the remote side, uploading files, port forwarding, SCP/SFTP.
- Parsing/understanding ANSI screen state and outputting a structured UI (raw bytes/text output is sufficient).
- "Automatically recognize the prompt/login flow for all devices and fully automate it." The first release provides tool capabilities and allows the caller to drive flows via `until_regex`.

## 4. User Stories & Typical Scenarios
- As a caller, I can open an SSH interactive session to a Linux machine via MCP, log in, run `docker exec -it ...`, and continuously read/write interactive output.
- As a caller, I can run a non-interactive command via `exec` on the same Linux machine and get an `exit_code` (e.g., `systemctl is-active xxx`).
- As a caller, I can log into a router via Telnet, enter `enable`/`config terminal`, paste multi-line configuration, and read the echo/output.
- As a caller, I can send Ctrl-C/Ctrl-D when things hang, or force-close the session.

## 5. MCP Tool Design (External API)
### 5.0 MCP Protocol Compliance
- **Protocol version**: implement MCP `2025-03-26` (InitializeRequest/InitializedNotification handshake).
- **Handshake order**: client sends `initialize`, server responds, client sends `notifications/initialized` before any `tools/*` or other requests.
- **STDIO transport**: newline-delimited JSON-RPC only; never write logs to stdout (use stderr for logs).
- **Streamable HTTP**: expose `/mcp` using MCP streamable HTTP (POST JSON-RPC, SSE for streaming); honor `MCP-Session-ID` headers as defined by the SDK.
- **Tool naming**: tool names must match `^[a-zA-Z0-9_-]+$` (no dots); publish `inputSchema` via `schemars`.
- **Auth (optional)**: when enabled, require `Authorization: Bearer <token>` and return `401` with `WWW-Authenticate: Bearer` on failure.

### 5.1 Conventions
- **Session**: `session_id` is an opaque string (e.g., UUID).
- **Output cursor (`cursor`)**: an opaque position marker for multi-consumer reads of the same session output; the caller stores `next_cursor` and passes it back on the next read, enabling tmux-like side-band monitoring without impacting the main AI flow.
- **Output encoding**: responses include an `encoding` field; default is `utf-8`. If undecodable bytes exist, `base64` may be returned.
- **Idempotency**: calling `close` multiple times should succeed (or return a dedicated "already closed" status).
- **Time unit**: milliseconds (ms).

### 5.2 Tool List (Core)
Tool names are suggestions; the concrete naming must satisfy the chosen MCP SDK/framework, but semantics and fields must remain consistent.

#### 5.2.1 `ptyctl_session`
Create, close, list, and lock sessions.

Input:
- `action`: `"open" | "close" | "list" | "lock" | "unlock" | "heartbeat" | "status"`
- `session_id`: string (required for close/lock/unlock/heartbeat/status)
- `force`: boolean (close only; default false)
- `lock_ttl_ms`: number (lock/heartbeat; default 60000)
- `task_id`: string (lock/unlock/heartbeat; required when locking)

Open-only fields (`action="open"`):
- `protocol`: `"ssh" | "telnet"`
- `host`: string
- `port`: number (SSH default 22; Telnet default 23)
- `username`: string (optional for Telnet; depends on device)
- `auth`: object (SSH only, optional; mainly for "auto-respond to common prompts"; any uncovered auth flow must be handled interactively)
  - `method`: `"password" | "private_key" | "agent" | "auto"`
  - `password`: string (when `method=password`)
  - `private_key_pem`: string (when `method=private_key`; PEM text; optional `passphrase`)
  - `passphrase`: string (optional)
- `pty`: object (whether to allocate an interactive PTY)
  - `enabled`: boolean (default true)
  - `cols`: number (default 120)
  - `rows`: number (default 40)
  - `term`: string (default `"xterm-256color"`)
- `timeouts`: object
  - `connect_timeout_ms`: number (default 15000)
  - `idle_timeout_ms`: number (default 0; 0 means no auto-disconnect; may be overridden by global configuration)
- `ssh_options`: object (SSH only, optional)
  - `host_key_policy`: `"strict" | "accept_new" | "disabled"` (default `"strict"`; maps to OpenSSH `StrictHostKeyChecking=yes|accept-new|no`)
  - `known_hosts_path`: string (optional)
  - `host_key_fingerprint`: string (optional; for pinning)
  - `use_openssh_config`: boolean (default true; when enabled, OpenSSH parses `~/.ssh/config`, `Include`, `ProxyJump`, etc.)
  - `config_path`: string (optional; passed to OpenSSH as `-F <path>`; only effective when `use_openssh_config=true`)
  - `extra_args`: string[] (optional; passed through to OpenSSH, e.g., `-J`/`-o`)
- `expect`: object (optional; helps determine "done" / "waiting for input"; should not be required for correctness)
  - `prompt_regex`: string (optional; indicates "ready for input / command done")
  - `pager_regexes`: string[] (optional; e.g., `--More--`, `\\(END\\)`)
  - `error_regexes`: string[] (optional; e.g., `(?i)(invalid input|error|failed|permission denied)`)
- `session_type`: `"normal" | "console"` (optional; default normal)
- `device_id`: string (required for console sessions)
- `acquire_lock`: boolean (optional; if true, lock on create)

Output:
- `action`: string
- `success`: boolean
- `session_id`: string (open/close)
- `protocol`: `"ssh" | "telnet"` (open)
- `pty_enabled`: boolean (open)
- `server_banner`: string (optional; e.g., SSH banner)
- `security_warning`: string | null (optional; Telnet should return a cleartext risk warning)
- `sessions`: array (list)
- `capabilities`: object (list)
- `lock_holder`: string (lock/status/heartbeat)
- `lock_expires_at`: number (lock/status/heartbeat)
- `lock_acquired`: boolean (open when acquire_lock=true)
- `existing_session_id`: string (open when console session already exists)

Errors:
- Connection/authentication/HostKey verification failures and timeouts must return a machine-readable `error_code` (see 7.1).

#### 5.2.2 `ptyctl_session_exec` (must support for SSH; Telnet may partially support)
Execute a "non-fullscreen / non-strongly-interactive" command and obtain its exit code.

Input:
- `session_id`: string
- `cmd`: string
- `timeout_ms`: number (default 60000)
- `until_idle_ms`: number (optional; mainly for Telnet best-effort: if the prompt is unknown, return once there is "no new output for N ms"; must be <= `timeout_ms`)
- `rc_mode`: object (optional)
  - `enabled`: boolean (default true; Telnet will typically degrade to best-effort)
  - `marker_prefix`: string (default `"\u001eRC="` i.e. starts with `0x1e`)
  - `marker_suffix`: string (default `"\u001f"` i.e. ends with `0x1f`)
- `expect`: object (optional; overrides session-level `expect`)
  - `prompt_regex`: string (optional)
  - `pager_regexes`: string[] (optional)
  - `error_regexes`: string[] (optional)

Output:
- `stdout`: string
- `stderr`: string (if the protocol/implementation cannot separate them, return empty and merge content into `stdout`, and declare this in `capabilities`)
- `exit_code`: number | null (return null if it cannot be obtained reliably, and provide `exit_code_reason`)
- `exit_code_reason`: string | null (e.g., `unsupported_shell` / `marker_not_seen` / `timeout` / `unknown`; may be null when `exit_code` has a value)
- `done_reason`: `"marker_seen" | "prompt_seen" | "idle_reached" | "timeout" | "eof" | "unknown"` (Telnet commonly: `prompt_seen/idle_reached/timeout/unknown`)
- `prompt_detected`: boolean | null (null if no `prompt_regex` is configured and no prompt has been learned)
- `error_hints`: string[] (optional; returned when matching `error_regexes`; helps caller/AI decide failure, but is not a hard verdict)
- `timed_out`: boolean
- `duration_ms`: number

Implementation requirements (SSH):
- Must return an exit code reliably. Either:
  - Use OpenSSH "remote command mode" process exit code as `exit_code` (no dependence on prompt/ANSI), or
  - Use an "end-marker protocol" parsed from output, e.g. inject: `<cmd>; rc=$?; printf "\n\x1eRC=%d\x1f\n" $rc`
- Must not depend on prompt, PS1, or ANSI.
- Automatic marker fallback (default behavior):
  - **Why**: some shells/PTY layers strip non-printable control characters (`0x1e/0x1f`), which can remove the default marker and make `exit_code` unavailable even though the command ran successfully.
  - **Logic**: when `rc_mode.enabled=true` and the caller does not override `marker_prefix/marker_suffix`, `ptyctl` emits two markers for the same command: the default control-character marker **and** a unique ASCII marker (e.g. `PTYCTL_RC_<uuid>=<rc>:END_<uuid>`). The read loop matches either marker and extracts the exit code from whichever appears first.
  - **Implementation**: append an extra `printf` that prints the ASCII marker using a per-exec UUID token, build a combined regex that matches either marker, and strip both markers from returned `stdout` before parsing. If a caller supplies custom markers, the fallback is disabled to avoid surprising output changes.

Implementation requirements (Telnet best-effort):
- If the remote shell/CLI supports "append marker output" (e.g., `echo`/`printf` plus command separators), allow `rc_mode` to obtain an exit code; otherwise return `exit_code=null`.
- Must return as much of the "raw echo/error text" as possible, and provide soft hints via `done_reason`/`error_hints` so that the AI can decide next steps like a human reading the terminal.

#### 5.2.3 `ptyctl_session_io`
Read/write bytes within an interactive session.

Input:
- `session_id`: string
- `action`: `"write" | "read"`

Write input:
- `data`: string (optional; provide either `data` or `key`)
- `key`: enum (optional; special keys)
  - `enter`
  - `tab`
  - `backspace`
  - `delete`
  - `home`
  - `end`
  - `ctrl_c`
  - `ctrl_d`
  - `ctrl_z`
  - `ctrl_backslash`
  - `ctrl_a`
  - `ctrl_e`
  - `ctrl_k`
  - `ctrl_u`
  - `ctrl_l`
  - `esc`
  - `arrow_up` / `arrow_down` / `arrow_left` / `arrow_right`
  - `page_up` / `page_down`
- `encoding`: `"utf-8" | "base64"` (default `utf-8`)
- `sensitive`: boolean (default false; when true, content must not be logged; if `record_tx_events` is enabled, only record an event like "sensitive write occurred")
- `task_id`: string (optional; required when a session is locked)

Read input:
- `mode`: `"cursor" | "tail"` (default `"cursor"`)
- `cursor`: string (optional; output cursor for independent reads in multi-client/monitoring scenarios. If omitted, start from the "current buffer end" and only return new output from that point on)
- `timeout_ms`: number (default 2000; max wait for this read call)
- `max_bytes`: number (default 65536)
- `max_lines`: number (tail mode only; optional)
- `until_regex`: string (optional; Rust regex; if matched, return early)
- `include_match`: boolean (default true)
- `until_idle_ms`: number (optional; read until "no new output for N ms"; often used for Telnet/device CLI without a known prompt; must be <= `timeout_ms`)
- `encoding`: `"utf-8" | "base64"` (default `utf-8`)
- `input_hints`: object (optional; for "waiting for input" detection to avoid pure guessing)
  - `wait_for_regexes`: string[] (e.g., `(?i)password:`, `\\(y/n\\)`)

Output:
- `action`: `"write" | "read"`
- `bytes_written`: number (write)
- `chunk`: string (read; for `mode="tail"` this is the tail snapshot)
- `encoding`: `"utf-8" | "base64"` (read)
- `next_cursor`: string (read; cursor to pass on the next read)
- `buffer_start_cursor`: string (read)
- `buffer_end_cursor`: string (read)
- `matched`: boolean (read)
- `idle_reached`: boolean (read)
- `timed_out`: boolean (read)
- `eof`: boolean (read)
- `waiting_for_input`: boolean (read)
- `truncated`: boolean (read)
- `dropped_bytes`: number (read)
- `buffered_bytes`: number (read)
- `buffer_limit_bytes`: number (read)

#### 5.2.4 `ptyctl_session_config`
Resize the remote PTY and manage expect configuration.

Input:
- `session_id`: string
- `action`: `"resize" | "expect" | "get"`
- `cols`: number (resize)
- `rows`: number (resize)
- `expect`: object (expect)
  - `prompt_regex`: string (optional)
  - `pager_regexes`: string[] (optional)
  - `error_regexes`: string[] (optional)

Output:
- `success`: boolean
- `cols`: number (get)
- `rows`: number (get)
- `expect`: object (get)

### 5.3 Capabilities
Capabilities are returned in `ptyctl_session` responses with `action="list"`:
- `supports_split_stdout_stderr` (ssh:true, telnet:false)
- `supports_exit_code` (ssh:true, telnet:best_effort)
- `supports_resize` (ssh:true, telnet:maybe)

## 6. Runtime Modes & Configuration
### 6.1 STDIO Mode
Used when Codex launches `ptyctl` as a local subprocess.
- Example: `ptyctl mcp --transport stdio`
- If you also want local side-band monitoring via `ptyctl attach/sessions/tail`, enable the local control endpoint (Unix socket) as well, e.g.: `ptyctl mcp --transport stdio --control-socket $XDG_RUNTIME_DIR/ptyctl.sock`
- Control socket fallback path: `$XDG_RUNTIME_DIR/ptyctl.sock` if available, otherwise `/run/user/<uid>/ptyctl.sock`, then `/tmp/ptyctl-<uid>.sock`.

### 6.2 HTTP Mode
Used for local/LAN access over HTTP (e.g., integrating into another system).
- Example: `ptyctl mcp --transport http --listen 127.0.0.1:8765`
- Must support configuring the bind address (default: localhost-only).
- Should support an optional `--auth-token` (simple bearer token) to prevent accidental exposure.
- Transport requirement: use the **MCP standard HTTP transport (HTTP + SSE)**. Default is **plain HTTP**; if deployment requires TLS, optional TLS is allowed (self-signed certificates are acceptable).

### 6.3 CLI Shape (suggested)
Provide a unified subcommand (alias is fine):
- `ptyctl serve` (equivalent to `ptyctl mcp`)
- Support `--transport stdio|http|both` (`both` means the same process exposes both transports)

Provide operational/debugging commands (for "tmux-like attach monitoring"):
- `ptyctl sessions`: list sessions (equivalent to calling `ptyctl_session` with `action="list"`)
- `ptyctl attach <session_id>`: attach and continuously print output (read-only by default; suggested flow: `ptyctl_session_io` `read` with `mode="tail"` then loop `read(cursor=...)`)
- `ptyctl tail <session_id>`: print a tail snapshot (equivalent to calling `ptyctl_session_io` with `action="read"` and `mode="tail"`)

### 6.4 Configuration Items (suggested)
Support CLI flags + a config file (e.g., `ptyctl.toml`), at minimum:
- Default timeouts: connect/read/exec/idle (e.g., `connect_timeout_ms`, `read_timeout_ms`, `default_exec_timeout_ms`, `idle_timeout_ms`; overridable per call)
- Max concurrent sessions and per-session buffer limits (prevent unbounded memory growth)
  - `max_sessions` (default suggested 100)
- Session output buffer strategy:
  - `output_buffer_max_bytes` (default suggested 2–8 MiB; hard cap; drop oldest output when exceeded and report via `ptyctl_session_io(read).truncated/dropped_bytes`)
  - `output_buffer_max_lines` (default suggested 20000; best-effort; lines split by `\\n`; still bounded by `output_buffer_max_bytes`)
- `record_tx_events` (default false; record TX events/placeholders only, not content)
- SSH HostKey policy: `strict|accept_new|disabled` (default strict; maps to OpenSSH `StrictHostKeyChecking=yes|accept-new|no`)
- Log level and log destination (stdout/stderr or file)

Recommended additions:
- `openssh_path` (default `"ssh"`, used to locate the OpenSSH client)
- `telnet_path` (default `"telnet"`, used to locate the system telnet client; ignored if using an in-process Telnet implementation)
- **Local control endpoint (for `ptyctl attach/sessions/tail`)**:
  - Rationale: the STDIO JSON-RPC connection is exclusively owned by the parent process and cannot be shared by another terminal concurrently. A tmux-like `attach` requires an additional local IPC channel.
  - Recommendation: provide a Unix Domain Socket (UDS) control endpoint, read-only by default, exposing only `ptyctl_session` with `action="list"` and `ptyctl_session_io` reads (cursor/tail) (no write/close, to avoid conflicts with the AI).
  - Suggested config:
    - `control_socket_path` (default: Linux `$XDG_RUNTIME_DIR/ptyctl.sock`, macOS `/tmp/ptyctl-$UID.sock`; fallback to `/tmp` when missing; CLI: `--control-socket <path>`)
    - `control_mode`: `"disabled" | "readonly" | "readwrite"` (default `"readonly"`; CLI: `--control-mode disabled|readonly|readwrite`)
    - (optional) `control_auth_token`: enable when `control_mode="readwrite"` or when listening on TCP

### 6.5 Configuration Example (inspired by `requirements.md`)
```toml
[server]
transport = "stdio" # stdio|http|both

[server.http]
listen = "127.0.0.1:8765"
auth_token = "" # optional

[server.control]
control_socket_path = "/tmp/ptyctl-$UID.sock" # example; prefer $XDG_RUNTIME_DIR
control_mode = "readonly" # disabled|readonly|readwrite
control_auth_token = "" # optional

[session]
max_sessions = 100
idle_timeout_ms = 300000
output_buffer_max_lines = 20000
output_buffer_max_bytes = 2097152
record_tx_events = false

[ssh]
openssh_path = "ssh"
use_openssh_config = true
config_path = "" # optional: equivalent to ssh -F <path>
host_key_policy = "strict" # strict|accept_new|disabled
known_hosts_path = "" # optional

[telnet]
telnet_path = "telnet"

[logging]
level = "info"
format = "text" # text|json
```

### 6.6 Environment Variables (inspired by `requirements.md`, optional)
- `PTYCTL_TRANSPORT=stdio|http|both`
- `PTYCTL_HTTP_LISTEN=127.0.0.1:8765`
- `PTYCTL_LOG_LEVEL=debug|info|warn|error`
- `PTYCTL_CONTROL_SOCKET=/tmp/ptyctl-$UID.sock`
- `PTYCTL_CONTROL_MODE=disabled|readonly|readwrite`

## 7. Error Handling & Return Conventions
### 7.1 Unified Error Codes (suggested)
- `INVALID_ARGUMENT`
- `NOT_FOUND` (session_id does not exist)
- `ALREADY_CLOSED`
- `CONNECT_TIMEOUT`
- `CONNECT_FAILED`
- `AUTH_FAILED`
- `HOSTKEY_MISMATCH`
- `IO_ERROR`
- `REMOTE_CLOSED`
- `EXEC_TIMEOUT`
- `UNSUPPORTED` (e.g., Telnet does not support exit_code)

Error returns should include:
- `error_code` (enum above)
- `message` (human-readable)
- `details` (optional: underlying error, phase, retryable or not)

### 7.2 Resource Cleanup
- On MCP server shutdown, best-effort close all sessions.
- When a session encounters an unrecoverable error, mark it as `error` and allow `close` to clean up.

### 7.3 MCP/JSON-RPC Error Payload (recommended; inspired by `requirements.md`)
Follow MCP error semantics (usually JSON-RPC errors). Recommend:
- Use standard JSON-RPC codes (-32600/-32601/-32602/-32603) or project-specific codes for `error.code`.
- Carry `error_code` (string enum above) and diagnostics in `error.data`, so callers are not limited to a generic "internal error".

Example:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": 1002,
    "message": "Authentication failed",
    "data": {
      "error_code": "AUTH_FAILED",
      "host": "192.168.1.1",
      "reason": "keyboard-interactive failed"
    }
  }
}
```

## 8. Security & Compliance
- **Never log passwords/private keys/tokens** (must be redacted).
- SSH defaults to strict HostKey checking; relaxing it must require explicit configuration.
- Telnet is cleartext: return a `security_warning` in `ptyctl_session` `action="open"` output (or in capabilities) so the caller can warn the user.
- HTTP mode should bind to `127.0.0.1` by default, unless the user explicitly configures a public bind.

## 9. Acceptance Criteria ("Definition of Done")
### 9.1 Functional Acceptance Cases
1. **SSH exec**: run `echo hello` against a local/test SSH server; return `stdout=hello` and `exit_code=0`.
2. **SSH interactive**: open a PTY, run `bash` in-session; after seeing a prompt, write `ls\n` and read output; sending `ctrl_c` should interrupt `sleep 999`.
3. **SSH docker scenario (simulated)**: run `docker exec -it` (or equivalent strongly interactive program) on a test machine; continuously read/write and eventually exit.
4. **Telnet basic interaction**: connect to a test telnet server, complete login if needed, send a command and read echo/output; return `timed_out=true` on timeout.
5. **Concurrency**: open N sessions concurrently (configurable, at least 100) and read/write on each without cross-talk.
6. **Timeout & cleanup**: auto-close sessions after `idle_timeout_ms` (when enabled), and remove/mark them in `ptyctl_session` `action="list"`.
7. **HTTP/STDIO dual mode**: the same toolset works in both transports and completes the above scenarios (at least cover 1, 2, 4).
8. **Controlled buffer overflow**: run a high-output command/scenario (e.g., print a large file); server must not crash or block the session; when dropping occurs, `ptyctl_session_io(read)` returns `truncated=true` and `dropped_bytes>0`.
9. **until-idle reads**: for Telnet/unknown prompt devices, `ptyctl_session_io(read)` with `until_idle_ms=...` returns after output becomes quiet and can be used to drive AI decisions.
10. **Force close**: when a session hangs or the underlying subprocess is unresponsive, `ptyctl_session` with `action="close"` and `force=true` frees resources and does not impact other sessions.

### 9.2 Quality Bar
- Long-running (e.g., 8 hours) without obvious memory leaks (create/close sessions repeatedly).
- A single session failure must not crash the whole process (fault isolation).

### 9.3 Performance & Resource Goals (targets; inspired by `requirements.md`)
- Max concurrent sessions: `max_sessions >= 100`
- Connection establishment time: < 5s under normal network conditions (target)
- Additional local read/write overhead: < 100ms (excluding network)
- Memory usage: baseline < 100MB; per-session overhead < 5MB (including default `output_buffer_max_bytes`)

## 10. Implementation Recommendations (non-mandatory, for execution)
- Rust async runtime: Tokio.
- SSH (recommended): **wrap the system OpenSSH client (`ssh`)** to reuse its authentication and configuration capabilities (`~/.ssh/config`, `Include`, `ProxyJump`, keyboard-interactive/MFA, etc.). `ptyctl` manages the local PTY and byte-stream I/O and exposes all prompts/echo to the caller.
  - Optional: enable OpenSSH `ControlMaster`/`ControlPersist` for frequent `ptyctl_session_exec` to reuse connections (must manage ControlPath lifecycle and cleanup).
- SSH (optional later): consider using a Rust SSH library for finer-grained channel control, but it must cover a large OpenSSH feature matrix.
- Telnet (recommended direction: protocol implementation, no local UI):
  - The goal is not a human telnet CLI UI (e.g., local `^]` command mode), but **Telnet protocol + async I/O + common negotiation** so the AI can drive the remote device by sending exactly what a human would type (commands and keys).
  - Recommended: **implement in-process Telnet (RFC 854) or integrate an existing Telnet codec**, manage the `TcpStream` inside the process, and stream "human-visible output (NVT data)" into the session ring buffer. Telnet negotiation bytes (IAC sequences) are consumed internally and must not appear in `ptyctl_session_io(read)` output. See `telnet-spec.md`.
  - Fallback: **wrap the system telnet client** (manage the subprocess via a local PTY) for environments without an in-process Telnet implementation or for quick validation; downside: depends on the system having `telnet` installed and may include some local client messages.
- MCP: use the official Rust SDK (RMCP) from https://github.com/modelcontextprotocol/rust-sdk. Implement MCP via the `rmcp` crate with `server` + `transport-io` for STDIO and `transport-streamable-http-server` for HTTP. Register tools using RMCP macros and expose JSON Schema for tool parameters using `schemars`.

## 11. Open Questions (Must Decide Before Starting Development)
1. Tool naming & schema
   - Decision: standardize on `ptyctl_session`, `ptyctl_session_exec`, `ptyctl_session_io`, and `ptyctl_session_config`.
2. How should the AI decide "command done / error" for Telnet?
   - Decision: Telnet `exit_code` is **best-effort** and not mandatory by default. `ptyctl`'s responsibility is to deliver as complete and timely "human-visible context" as possible, plus a few soft-hint fields.
   - Concrete strategy:
     - **Prefer markers**: when the target is clearly a POSIX shell (or the AI has inferred it), allow `rc_mode` on Telnet to fetch exit codes by appending markers; otherwise return `exit_code=null`.
     - **Prompt as "done state"**: the AI observes the prompt via an initial `ptyctl_session_io(read)`, then calls `ptyctl_session_config` with `action="expect"` to set `prompt_regex`; afterwards `ptyctl_session_exec` uses `done_reason=prompt_seen` as the primary signal that the previous command finished.
     - **until-idle as fallback**: when prompt extraction is hard, use `ptyctl_session_io(read)` with `until_idle_ms=...` or `ptyctl_session_exec` with `until_idle_ms=...`, treating "output silence" as an end-of-phase weak signal (AI must decide with context).
     - **Soft error hints**: produce `error_hints` via configurable `error_regexes` (e.g., `Invalid input`, `%Error`, `Permission denied`); avoid hard failure decisions inside `ptyctl`.
     - **Pager/confirm prompts**: `pager_regexes` helps the AI recognize "not done; waiting for paging/confirmation" so it can send `space/q/y/n` etc.
3. SSH authentication & OpenSSH config reuse
   - Decision: must support password/private key/agent/keyboard-interactive/MFA and be compatible with `ProxyJump`; prefer wrapping OpenSSH to reuse its config parsing and auth capabilities.
   - Requirements:
     - Read OpenSSH config by default (`~/.ssh/config`, `Include`, system config); allow `ssh_options.config_path` to specify a path.
     - Allow `ssh_options.extra_args` passthrough when needed (e.g., `-J`, `-o`).
     - Interactive auth prompts (MFA/keyboard-interactive/first-time hostkey confirmation) must be surfaced via `ptyctl_session_io(read)` and responded to via `ptyctl_session_io(write)` using `data` or `key`.
4. HTTP transport & TLS
   - Decision: standard HTTP is enough; HTTPS is not required. If deployment requires TLS, allow self-signed certs (optional capability).
5. Target platforms
   - Decision: support Linux/macOS first; Windows later.

## 12. Output Stream & Buffering Strategy (AI-friendly; must satisfy)
### 12.1 Why Server-side Buffering Is Needed
PTYs/pipes have kernel buffers. If `ptyctl` does not continuously read, the subprocess (`ssh`/`telnet`) can block once its output fills the buffer, causing the remote interaction to "freeze". Therefore:
- `ptyctl` must start a background read task per session, continuously reading from the PTY master and writing into a **bounded ring buffer**.
- `ptyctl_session_io` reads (cursor or tail mode) must read from the ring buffer only, and must not rely on caller read frequency to drive the underlying reads.

### 12.2 Buffer Units & Limits
- Buffering is bounded by **bytes** (compatible with ANSI/unprintable/non-UTF8 bytes).
- Optionally provide a **line-based soft cap** (split by `\\n`, best-effort) for "tail N lines" windows that are easier for AI consumption.
- Treat output as an "append-only log stream", and provide `cursor/next_cursor` semantics for multi-consumer reads (side-band monitoring/attach).

Suggested defaults (configurable):
- `output_buffer_max_lines = 20000`
- `output_buffer_max_bytes = 2 MiB` (with `max_sessions=100`, this is ~200 MiB max; tune based on machine memory)

### 12.3 Overflow Behavior (Must Be Observable)
When output exceeds buffer limits:
- Must drop the **oldest** data (keep the latest output visible).
- Must report dropping via `ptyctl_session_io(read).truncated` and `dropped_bytes`.
- The caller/AI should react by reading more frequently, reducing command output (e.g., `--no-pager`/`head`/filters), or using more structured output options (e.g., `--json`).

### 12.4 Should We Record Input (TX) in the Server Buffer? (Optional)
"Input (TX)" means bytes sent to the PTY by `ptyctl_session_io(write)` using `data` or `key`. There are two modes:
- **Decision (confirmed): buffer only PTY output (RX)**. Rationale: most devices echo commands, so AI can see what it typed from RX; password inputs are typically not echoed and should not be stored server-side.
- **Optional enhancement (confirmed): record TX "events/placeholders"** (e.g., "sent Ctrl-C", "sent 12 bytes" without recording content) to aid debugging or to help reconstruct context when the device does not echo. Suggested config: `record_tx_events` (default false).

If we ever need to store TX plaintext in the future, requirements must include:
- Allow per-write `sensitive=true` (do not buffer, do not log).
- Default off, and document the risk clearly (may contain passwords/keys/tokens).

## 13. Appendix (inspired by `requirements.md`)
### 13.1 Common Control Keys & Sequences (for `ptyctl_session_io` with `action="write"` + `key`)
| `key` | Byte sequence (conceptual) |
|---|---|
| `ctrl_c` | `\\x03` |
| `ctrl_d` | `\\x04` |
| `ctrl_z` | `\\x1a` |
| `ctrl_backslash` | `\\x1c` |
| `tab` | `\\x09` |
| `enter` | `\\r` (implementation may choose `\\n`, but `\\r` is recommended for device CLI compatibility) |
| `esc` | `\\x1b` |
| `arrow_up` | `\\x1b[A` |
| `arrow_down` | `\\x1b[B` |
| `arrow_right` | `\\x1b[C` |
| `arrow_left` | `\\x1b[D` |
| `home` | `\\x1b[H` |
| `end` | `\\x1b[F` |
| `backspace` | `\\x7f` |
| `delete` | `\\x1b[3~` |
| `page_up` | `\\x1b[5~` |
| `page_down` | `\\x1b[6~` |

## 14. Test Strategy (inspired by `requirements.md`, suggested)
- Unit tests: ring buffer overflow behavior, `until_regex`/`until_idle_ms`, error code mapping, key sequence mapping.
- Integration tests: run a local `sshd` (or container) to verify SSH `ptyctl_session_exec`/interactive/force-close; run a mock telnet server to verify Telnet `ptyctl_session_io` read/write/until-idle.
- Scenario tests: `sudo` prompts, pagers (`less/more`), `docker exec -it`, router `enable/config terminal` (can be recorded replay or simulated server).

## 15. Typical Interaction Flows (inspired by `requirements.md`)
### 15.1 `sudo` scenario
- Prefer non-interactive paths: use non-interactive flags (e.g., `--non-interactive`, `-y`), or use an account with sufficient privileges, reducing `sudo` password prompts.
- If a password is required:
  1. `ptyctl_session_io` with `action="write"` and `data="sudo <cmd>\\r"`
  2. `ptyctl_session_io` with `action="read"` and `input_hints.wait_for_regexes=["(?i)password:"]`
  3. `ptyctl_session_io` with `action="write"` and `data="<password>\\r"`, `sensitive=true`
  4. `ptyctl_session_io` with `action="read"` and `until_regex="<prompt_regex>"` or `until_idle_ms=...`

### 15.2 `docker exec -it` scenario (SSH interactive)
1. `ptyctl_session_io` with `action="write"` and `data="docker exec -it <container> /bin/bash\\r"`
2. `ptyctl_session_io` with `action="read"` and `until_regex="<container_prompt_regex>"` or `until_idle_ms=...`
3. Continue interactive commands inside the container
4. `ptyctl_session_io` with `action="write"` and `data="exit\\r"`, then read until returning to the host prompt

### 15.3 Router configuration mode (Telnet/SSH interactive)
1. `ptyctl_session` with `action="open"` and `protocol="telnet"`
2. `ptyctl_session_io` with `action="read"` and `until_regex="(>|#|login:|username:)"` or `until_idle_ms=...`
3. `ptyctl_session_io` with `action="write"` and `data="enable\\r"` → wait for password prompt → `ptyctl_session_io(write, sensitive=true)`
4. `ptyctl_session_io` with `action="write"` and `data="configure terminal\\r"` → `ptyctl_session_io(read)` with `until_regex="\\(config\\)#"` or `until_idle_ms=...`
5. Send configuration lines and read echo; use `pager_regexes` if paging occurs
6. `ptyctl_session_io` with `action="write"` and `data="end\\r"` / `data="write memory\\r"`

## 16. Session Monitoring & "Attach" (Operations capability)
### 16.1 Goals
When `ptyctl` maintains multiple SSH/Telnet sessions, besides the AI, operators/developers should be able to use it like `screen/tmux` to:
- View which sessions exist and their state (open/closed/error, last activity, etc.).
- Attach read-only to a session and observe output in real time (without disturbing the AI).
- Fetch a tail snapshot for troubleshooting.

### 16.2 Mechanism
- The server maintains a bounded ring buffer per session (see section 12) and exposes `cursor/next_cursor` semantics:
  - The monitor first calls `ptyctl_session_io` with `action="read"` and `mode="tail"` to get the latest tail and `next_cursor`.
  - Then it loops `ptyctl_session_io` with `action="read"` and `cursor=next_cursor` to follow new output.
  - Multiple monitors use their own cursors and do not "steal" output or affect the AI.

### 16.3 Recommended Behavior
- By default, `ptyctl` provides read-only attach (no input) to avoid concurrent writes with the AI causing confusion.
- If the main flow uses STDIO transport and you want side-band attach from another terminal, enable the local control endpoint (recommended: Unix socket). `ptyctl attach/sessions/tail` should connect to that endpoint for read-only monitoring (without needing to "reuse" STDIO).
- If interactive attach is needed in the future, define an explicit control/ownership mechanism (acquire/release input lock) and conflict policies (priority, timeout, forced takeover, etc.).
