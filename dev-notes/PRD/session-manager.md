## ptyctl Session Management — PRD & Implementation Guide

### Overview

This document defines the **client-facing contract** and **recommended usage patterns** for ptyctl session management and IO. It is written as an implementation and integration guide for callers using MCP tools (STDIO/HTTP).

The session system is organized around **four primary tools**:

- **`ptyctl_session`**: session lifecycle + locking
- **`ptyctl_session_exec`**: command execution within an existing session
- **`ptyctl_session_io`**: unified read/write interface (cursor + tail)
- **`ptyctl_session_config`**: resize + expect configuration

---

### Background and goals

The system is designed for interactive remote control over **SSH** and **Telnet** sessions with a bounded output buffer and optional lock-based single-writer control.

#### Goals

- **Simple tool surface area**: callers use a small, consistent tool set for lifecycle, IO, execution, and configuration.
- **Safe concurrency**: prevent multiple concurrent writers from corrupting interactive sessions via **task-scoped locks**.
- **Console support**: enforce “single-writable per device” semantics with **Console sessions**.
- **Token-efficient reads**: support **cursor-based incremental reads** to avoid repeated content.

#### Non-goals

- **Universal prompt automation**: prompt/pager detection is best-effort and caller-configurable; correctness must not depend on it.
- **Full terminal UI modeling**: output is returned as text (or base64) chunks; ANSI screen state is not reconstructed.

---

### Definitions and concepts

- **Session**: a live SSH/Telnet connection identified by `session_id` (opaque string).
- **SessionType**:
  - **`normal`**: regular sessions; write is allowed when unlocked.
  - **`console`**: uniquely indexed by `device_id`; **requires a lock for write access**.
- **Task**: a caller-provided identifier (`task_id`) representing the single writer.
- **Lock**: a per-session write lease bound to `task_id` with an expiry (TTL).
- **Cursor**: an opaque position (stringified integer) in the output buffer used for incremental reads.
- **Output buffer**: bounded by server configuration; if overflow occurs, oldest bytes are dropped and reads may return `truncated=true` with `dropped_bytes>0`.

---

### Tool overview

#### `ptyctl_session` (lifecycle + locking)

Provides actions:

- `open`, `close`, `list`
- `lock`, `unlock`, `heartbeat`, `status`

#### `ptyctl_session_exec` (execute command)

Executes a command inside a session and (when enabled) returns an exit code via an injected marker protocol.

#### `ptyctl_session_io` (read/write)

Unified read/write:

- `write`: send `data` or a `key`
- `read`: cursor mode (incremental) or tail mode (snapshot)

#### `ptyctl_session_config` (resize/expect/get)

Session configuration:

- `resize`: set PTY size
- `expect`: set prompt/pager/error patterns
- `get`: fetch current size + expect config

---

### API: `ptyctl_session` (Lifecycle Management)

#### Actions

| Action | Purpose |
|--------|---------|
| `open` | Create a new session, or return an existing console session for a `device_id` |
| `close` | Close a session |
| `list` | List active sessions + capabilities |
| `lock` | Acquire (or extend) a write lock |
| `unlock` | Release a write lock |
| `heartbeat` | Extend lock expiry without changing lock ownership |
| `status` | Query lock holder + expiry |

#### Request schema (conceptual)

```rust
pub struct SessionRequest {
    pub action: SessionAction,

    // open parameters
    pub protocol: Option<Protocol>,      // ssh | telnet
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth: Option<SshAuth>,
    pub pty: Option<PtyOptions>,
    pub timeouts: Option<Timeouts>,
    pub ssh_options: Option<SshOptions>,
    pub expect: Option<ExpectConfig>,
    pub session_type: Option<SessionType>,  // normal | console
    pub device_id: Option<String>,          // required for console
    pub acquire_lock: Option<bool>,         // if true, auto-lock on create
    pub lock_ttl_ms: Option<u64>,           // default 60000
    pub task_id: Option<String>,            // required when acquiring/using locks

    // close/lock parameters
    pub session_id: Option<String>,
    pub force: Option<bool>,
}
```

#### Response schema (conceptual)

```rust
pub struct SessionResponse {
    pub action: SessionAction,
    pub success: bool,

    // open response
    pub session_id: Option<String>,
    pub protocol: Option<Protocol>,
    pub pty_enabled: Option<bool>,
    pub security_warning: Option<String>,
    pub lock_acquired: Option<bool>,
    pub existing_session_id: Option<String>,

    // list response
    pub sessions: Option<Vec<SessionListEntry>>,
    pub capabilities: Option<Capabilities>,

    // lock/status response
    pub lock_holder: Option<String>,
    pub lock_expires_at: Option<u64>, // epoch ms

    pub message: Option<String>,
}
```

#### Behavioral requirements

- **`open`**
  - **`protocol`** and **`host`** are required.
  - **Console sessions**:
    - If `session_type="console"`, **`device_id` is required**.
    - If a console session for `device_id` already exists, the tool returns that existing session:
      - `session_id` is the existing session id
      - `existing_session_id` is set (same value)
  - **`acquire_lock=true`**
    - Requires `task_id`.
    - If a **new session is created**, the lock is acquired and `lock_acquired=true`.
    - If `open` returns an existing console session, `lock_acquired=false` (caller must `lock` explicitly).
  - **Telnet**: returns `security_warning` indicating cleartext transport risk.

- **`close`**
  - Requires `session_id`.
  - `force=true` requests a forced close (implementation-defined at backend level).
  - If `session_id` does not exist, the server returns a not-found error; callers may treat this as “already closed” for cleanup flows.

- **`list`**
  - Returns:
    - `sessions`: active sessions (including `session_type` and `device_id`)
    - `capabilities`: capability flags

- **`lock` / `unlock` / `heartbeat` / `status`**
  - Require `session_id`.
  - `lock`, `unlock`, and `heartbeat` require `task_id`.
  - `lock_ttl_ms` defaults to **60000 ms** (60s) when omitted.
  - `lock_holder` and `lock_expires_at` are returned on `lock` and `heartbeat`.

#### Examples (parameters only)

```json
{"action":"open","protocol":"ssh","host":"192.0.2.10","username":"admin"}
```

```json
{"action":"open","protocol":"telnet","host":"console.local","session_type":"console","device_id":"switch-001","acquire_lock":true,"task_id":"task-123"}
```

```json
{"action":"lock","session_id":"<session_id>","task_id":"task-123","lock_ttl_ms":60000}
```

```json
{"action":"heartbeat","session_id":"<session_id>","task_id":"task-123"}
```

```json
{"action":"status","session_id":"<session_id>"}
```

---

### API: `ptyctl_session_io` (Unified Input/Output)

#### Request schema (conceptual)

```rust
pub struct SessionIoRequest {
    pub session_id: String,
    pub action: IoAction, // write | read

    // write
    pub data: Option<String>,        // with optional encoding
    pub key: Option<SessionKey>,     // special keys (ctrl_c, enter, arrows, ...)
    pub encoding: Option<Encoding>,  // utf-8 (default) | base64
    pub sensitive: Option<bool>,     // mark write as sensitive (no payload logging)

    // read
    pub mode: Option<ReadMode>,      // cursor (default) | tail
    pub cursor: Option<String>,      // for cursor mode incremental reads
    pub timeout_ms: Option<u64>,
    pub max_bytes: Option<usize>,
    pub max_lines: Option<usize>,    // tail mode only
    pub until_regex: Option<String>,
    pub include_match: Option<bool>,
    pub until_idle_ms: Option<u64>,
    pub input_hints: Option<InputHints>,

    // lock validation (write only; also used by exec)
    pub task_id: Option<String>,
}
```

#### Response schema (conceptual)

```rust
pub struct SessionIoResponse {
    pub action: IoAction,

    // write
    pub bytes_written: Option<usize>,

    // read
    pub chunk: Option<String>,
    pub encoding: Option<Encoding>,
    pub next_cursor: Option<String>,
    pub buffer_start_cursor: Option<String>,
    pub buffer_end_cursor: Option<String>,
    pub matched: Option<bool>,
    pub idle_reached: Option<bool>,
    pub timed_out: Option<bool>,
    pub eof: Option<bool>,
    pub waiting_for_input: Option<bool>,
    pub truncated: Option<bool>,
    pub dropped_bytes: Option<u64>,
    pub buffered_bytes: Option<usize>,
    pub buffer_limit_bytes: Option<usize>,
}
```

#### Write requirements

- `action="write"` requires exactly one of:
  - `data` (text or base64, controlled by `encoding`)
  - `key` (special key press)
- **Lock enforcement**
  - If the session is locked, `task_id` must be provided and must match the lock holder.
  - If the session is a **console session**, writes require a lock even if none is currently held (caller must `lock` first).
- If writing credentials or other secrets, set `sensitive=true` so the server avoids logging payload content.

#### Read requirements and semantics

- `action="read"` supports:
  - **Cursor mode** (`mode="cursor"` or omitted): incremental reads from `cursor`
  - **Tail mode** (`mode="tail"`): snapshot of the end of the buffer (optionally limited by `max_lines`)
- **Cursor as opaque**: callers should store `next_cursor` and reuse it for subsequent reads.
- **Buffer truncation detection**:
  - If a caller’s `cursor` is older than the buffer start, the response sets `truncated=true` and `dropped_bytes>0`.
  - Callers should treat this as data loss and recover by using tail mode to re-sync.
- `until_regex` and `until_idle_ms` are optional stopping conditions for interactive loops.
- `input_hints.wait_for_regexes` enables `waiting_for_input=true` when the returned chunk suggests the session is prompting for input (best-effort).

#### Examples (parameters only)

```json
{"session_id":"<session_id>","action":"write","data":"show version\n","task_id":"task-123"}
```

```json
{"session_id":"<session_id>","action":"write","key":"ctrl_c","task_id":"task-123"}
```

```json
{"session_id":"<session_id>","action":"read","cursor":"1234","until_idle_ms":500}
```

```json
{"session_id":"<session_id>","action":"read","mode":"tail","max_lines":20}
```

---

### API: `ptyctl_session_exec` (Command Execution)

`ptyctl_session_exec` is optimized for “command-style” interactions within an existing session.

#### Key behaviors

- **Write access is required**: `task_id` must be provided when the session is locked, and console sessions require a lock for exec.
- Exit code extraction is done by appending an exit-code marker command sequence when `rc_mode.enabled` is true.
  - `stdout` contains command output with markers removed (best-effort).
  - `exit_code` may be `null` if unsupported or not extracted.

#### Example (parameters only)

```json
{"session_id":"<session_id>","cmd":"uname -a","timeout_ms":15000,"task_id":"task-123"}
```

---

### API: `ptyctl_session_config` (Resize / Expect / Get)

#### Behaviors

- `resize`: requires `cols` and `rows`. Applies to sessions that have PTY enabled.
- `expect`: updates the session’s expect configuration (prompt/pager/error regexes).
- `get`: returns current expect configuration and (if PTY enabled) current `cols`/`rows`.

#### Examples (parameters only)

```json
{"session_id":"<session_id>","action":"resize","cols":120,"rows":40}
```

```json
{"session_id":"<session_id>","action":"expect","expect":{"prompt_regex":".+[>#]\\s*$","pager_regexes":["--More--"],"error_regexes":["(?i)(error|failed|invalid)"]}}
```

```json
{"session_id":"<session_id>","action":"get"}
```

---

### Locking and concurrency model

#### Core rules

- **Read operations never require a lock**.
- **Write operations** (`ptyctl_session_io` with `write`, and `ptyctl_session_exec`) enforce:
  - If a lock exists: `task_id` must match the lock holder.
  - If no lock exists:
    - `normal` sessions: write is allowed.
    - `console` sessions: write is rejected; caller must acquire a lock first.

#### Lock TTL and heartbeat

- Default TTL is **60 seconds**.
- Callers should heartbeat at ~50% TTL (e.g., every 30s) while actively controlling a session.
- Locks are automatically treated as released after expiry (session remains open).

#### Console session uniqueness

- For `session_type="console"`, **only one session per `device_id`** is maintained.
- `open` is effectively idempotent for console sessions:
  - If the console session exists, it is returned instead of creating a new one.

---

### Recommended client workflows

#### Workflow A: normal interactive session (single agent)

- `ptyctl_session` `open` (normal session)
- Loop:
  - `ptyctl_session_io` `write` to send commands/keys
  - `ptyctl_session_io` `read` using cursor mode and `until_idle_ms` to collect output incrementally

Tip: use **tail mode once** after connecting to get immediate context, then continue with cursor mode using `next_cursor`.

#### Workflow B: exclusive console control (single writer + observers)

- Writer:
  - `ptyctl_session` `open` with `session_type="console"` and `device_id`
  - `ptyctl_session` `lock` with `task_id`
  - Heartbeat loop:
    - `ptyctl_session` `heartbeat` periodically
  - Use `ptyctl_session_io(write)` and `ptyctl_session_exec` with matching `task_id`
  - `ptyctl_session` `unlock` when done

- Observers:
  - Use `ptyctl_session_io(read)` (tail or cursor) without a lock.

#### Workflow C: handling lock contention

- If a writer receives an error like “Session locked by task X”:
  - Treat the session as **read-only**
  - Continue reading output to stay in sync
  - Retry lock acquisition later or fail fast depending on caller policy

#### Workflow D: handling buffer truncation

- If a read returns `truncated=true` and `dropped_bytes>0`:
  - Consider output lost; surface a warning to the caller
  - Re-sync by using tail mode, then continue with the returned `next_cursor`

---

### Acceptance criteria (behavioral)

- **Lock correctness**
  - When locked, only the lock holder (`task_id`) can write/exec.
  - Lock TTL expires and releases ownership automatically.
  - Console sessions reject writes unless locked.

- **Console uniqueness**
  - Multiple `open` calls for the same `device_id` return the same session id.

- **Cursor reads**
  - Incremental reads return `next_cursor` and buffer cursor metadata.
  - Buffer overflow is detectable via `truncated` / `dropped_bytes`.
