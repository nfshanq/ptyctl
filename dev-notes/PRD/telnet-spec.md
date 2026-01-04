# ptyctl Telnet Adapter Specification (Protocol + Async I/O + Common Negotiation)

This document defines the implementation requirements and boundaries for the **Telnet (RFC 854) protocol adapter** inside `ptyctl`. The goal is to let the AI operate remote device CLIs like a human: the AI only needs to send commands and key presses that a human would type, while `ptyctl` handles Telnet protocol details, negotiation, async streaming output, and multi-session concurrency.

> Note: This document does not require implementing a "human-facing telnet client UI" (e.g., local `^]` command mode, interactive menus, local line editing, etc.). Those belong to the client UX layer and are not `ptyctl`'s goal.

## 1. Goals and Non-goals

### 1.1 Goals
- Support `ptyctl_session` with `action="open"` and `protocol="telnet"` to create a Telnet session and complete basic negotiation.
- Support 100 concurrent sessions: one independent connection per session, one background read loop per session, and a bounded output buffer per session.
- `ptyctl_session_io` with `action="read"` returns "human-visible output" (NVT data) and **must not** include Telnet protocol control bytes (IAC sequences).
- Support common negotiation options (see section 4) for broad compatibility with network device/router CLIs.
- Support streaming reads: `ptyctl_session_io` with `action="read"` can be called repeatedly to continuously fetch output; the server must drain the socket in the background to avoid the remote side blocking due to full buffers.

### 1.2 Non-goals
- Do not implement a local command mode (e.g., netkit's `^]` escape commands).
- Do not implement file transfer protocols (Kermit/ZMODEM, etc.).
- Do not implement Telnet encryption/authentication extensions (Telnet is cleartext; use SSH for security).

## 2. Terminology and Byte Constants

- **NVT**: Network Virtual Terminal; Telnet "application data" stream (human-visible output/input).
- **IAC**: Interpret As Command (`0xFF`), indicating that the following bytes are Telnet commands/negotiation.

Common command bytes (RFC 854):
- `IAC` = `0xFF`
- `DONT` = `0xFE`
- `DO` = `0xFD`
- `WONT` = `0xFC`
- `WILL` = `0xFB`
- `SB` = `0xFA` (Subnegotiation Begin)
- `SE` = `0xF0` (Subnegotiation End; encoded as `IAC SE`)

## 3. Session Model & Async I/O

### 3.1 Connection and task model
Each Telnet session includes at least:
- a `TcpStream` (or split into `OwnedReadHalf/OwnedWriteHalf`);
- a background read loop that continuously reads from the socket, parses Telnet protocol bytes, and writes NVT data into the session ring buffer;
- an optional write queue (to serialize writes and avoid concurrent write conflicts).

**Key requirement**: the background read loop must continuously drain. `ptyctl_session_io` with `action="read"` must read from the ring buffer only and must not rely on caller frequency to drive underlying socket reads (otherwise the session may stall).

### 3.2 Output buffering
Follow the ring buffer strategy in `requestments-codex.md`:
- byte hard limit: `output_buffer_max_bytes`;
- line soft limit: `output_buffer_max_lines` (best-effort);
- on overflow, drop the oldest data; make it observable via `ptyctl_session_io(read).truncated/dropped_bytes`.

## 4. Telnet Negotiation: Supported Scope and Strategy

### 4.1 Principles
- `ptyctl` must handle remote-initiated negotiation (WILL/WONT/DO/DONT) and respond stably to avoid negotiation loops.
- Negotiation bytes (IAC sequences) **must not** appear in `ptyctl_session_io(read)` output.
- Implement RFC 1143 (Q Method) or an equivalent negotiation state machine to prevent repeated/cycling negotiation.

### 4.2 Required options (recommended minimal set)
The table below lists the options that the adapter must respond to/handle correctly (option code in parentheses).

| Option | Purpose | Handling requirement |
|---|---|---|
| BINARY (0) | 8-bit transparent transport | Allow negotiation; at minimum respond correctly and avoid loops; forcing enablement should be configurable |
| ECHO (1) | Which side performs echo | Default: no local echo; accept remote echo (common device behavior) |
| SGA (3) | Suppress Go Ahead | Accept (most devices use it) |
| TTYPE (24) | Terminal type | Support subnegotiation (SEND/IS) and configurable terminal type |
| NAWS (31) | Window size | Support subnegotiation and update with `ptyctl_session_config` `action="resize"` |

Recommended support (enable when required by a device):
- LINEMODE (34): default reject or best-effort (most network devices do not need it).
- NEW-ENVIRON (39) / ENVIRON (36): default reject/ignore (can be extended later).

### 4.3 Negotiation response rules (example policy)
Telnet negotiation is bidirectional: the remote may ask "we should do something" (`DO <opt>`) and may also declare "it will do something" (`WILL <opt>`).

Recommended default policy (configurable):
- Remote sends `WILL ECHO`: reply `DO ECHO` (accept remote echo).
- Remote sends `DO ECHO`: reply `WONT ECHO` (we do not perform local echo).
- Remote sends `DO NAWS`: reply `WILL NAWS`, then immediately send `SB NAWS <w><h> IAC SE`.
- Remote sends `DO TTYPE`: reply `WILL TTYPE`; after receiving `SB TTYPE SEND IAC SE`, reply `SB TTYPE IS <term> IAC SE`.
- For unknown/unsupported options: consistently reject (`DONT`/`WONT`) and avoid repeatedly sending responses that would cause loops.

## 5. Subnegotiation Details

### 5.1 NAWS (RFC 1073)
When NAWS is enabled, send:
`IAC SB NAWS <width_hi> <width_lo> <height_hi> <height_lo> IAC SE`

Notes:
- Width/height are 16-bit unsigned integers (network byte order).
- If subnegotiation data contains byte `0xFF`, it must be escaped as IAC (write `0xFF 0xFF`).
- When `ptyctl_session_config` with `action="resize"` is called and NAWS is enabled for the session, send an updated NAWS message.

### 5.2 TTYPE (RFC 1091)
Typical flow:
1. Negotiate: remote `DO TTYPE` → we respond `WILL TTYPE`
2. Remote request: `IAC SB TTYPE SEND IAC SE`
3. Our response: `IAC SB TTYPE IS <term-bytes> IAC SE`

Requirements:
- Default `<term-bytes>` comes from `session_open.pty.term` (e.g., `xterm-256color`) and should be overridable by configuration.
- The remote may send `SEND` multiple times; the implementation should respond stably each time.
- Apply the IAC escaping rule when encoding subnegotiation payload.

## 6. Byte Stream Parser (IAC State Machine)

### 6.1 Cases that must be handled correctly
- Network reads may split an IAC sequence across multiple `read()` calls; the parser must maintain state across chunks.
- `IAC IAC` represents literal byte `0xFF` (NVT data) and must be emitted into the ring buffer.
- `IAC SB ... IAC SE`: collect subnegotiation bytes correctly and trigger the corresponding handler.
- `IAC DO/WILL/WONT/DONT <opt>`: trigger the negotiation state machine and send responses.

### 6.2 Output rules
Parsing produces two types of outputs:
- **NVT data**: written into the session ring buffer, returned to the AI via `ptyctl_session_io(read)`.
- **Protocol events**: negotiation/subnegotiation events for internal handling and observability (optional: debug logs/metrics), not returned directly to the AI.

## 7. Input (Write) Behavior and Newlines

### 7.1 `ptyctl_session_io` write with `key="enter"` recommendation
For device CLI compatibility, `enter` should send `\\r` (CR). This is also recommended in the key table in `requestments-codex.md`.

### 7.2 `ptyctl_session_io` write newline normalization (recommended)
To reduce AI cognitive load (it often writes `cmd\\n`), provide a session-level configuration:
- `telnet_line_ending = "cr" | "crlf" | "lf" | "pass_through"`
  - Default `cr`: normalize single-byte `\\n` in written data to `\\r` (only for `encoding=utf-8` writes)
  - `pass_through`: do not rewrite bytes

Constraints:
- For `encoding=base64` writes, do not rewrite any bytes.
- If `sensitive=true`, do not log content; if `record_tx_events` is enabled, only record an event like "sensitive write occurred".

## 8. Error Handling and Observability

### 8.1 Error mapping
Telnet must cover at least:
- `CONNECT_TIMEOUT` / `CONNECT_FAILED`
- `IO_ERROR`
- `REMOTE_CLOSED`
- `NOT_FOUND` (invalid session)

### 8.2 Metrics/logging (recommended)
Per session, consider recording:
- negotiation event counters (DO/WILL/SB, etc.)
- cumulative sent/received bytes
- dropped bytes due to buffer overflow (`dropped_bytes_total`)

## 9. Test Cases (recommended)

### 9.1 Unit tests (parser/negotiation)
- IAC sequences split across chunks are parsed correctly.
- `IAC IAC` results in a single output byte `0xFF`.
- NAWS/TTYPE subnegotiation encoding and IAC escaping are correct.
- The negotiation state machine prevents loops (repeated DO/WILL for the same option does not cause infinite back-and-forth).

### 9.2 Integration tests (mock server)
Implement a minimal Telnet mock server (Tokio) for CI:
- Proactively send `DO TTYPE`, `DO NAWS`, `WILL ECHO`, `WILL SGA` and validate the client's responses.
- Send data containing IAC escapes and verify `ptyctl_session_io(read)` output is correct and contains no protocol bytes.
- Simulate disconnects/half-closes and verify `REMOTE_CLOSED` and resource cleanup.
