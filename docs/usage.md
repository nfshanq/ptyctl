# ptyctl MCP Usage (after install)

This guide focuses on how to use ptyctl after it is installed: connecting from Codex/VSCode, working with sessions, and monitoring LLM activity from the CLI.

If you still need installation steps, see `README.md`.

## 1) Start the MCP server

Pick the transport you want to use:

```bash
ptyctl serve --transport stdio
# or
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
# or
ptyctl serve --transport both --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

## 2) Connect from Codex

### STDIO

```bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

### HTTP

```bash
export PTYCTL_AUTH_TOKEN=YOUR_TOKEN
codex mcp add ptyctl-http \
  --url http://127.0.0.1:8765/mcp \
  --bearer-token-env-var PTYCTL_AUTH_TOKEN
```

## 3) Connect from VSCode/Cursor

### STDIO

```json
{
  "mcpServers": {
    "ptyctl-stdio": {
      "command": "/usr/local/bin/ptyctl",
      "args": ["serve", "--transport", "stdio"],
      "env": {
        "PTYCTL_LOG_LEVEL": "info"
      }
    }
  }
}
```

### HTTP

```json
{
  "mcpServers": {
    "ptyctl-http": {
      "url": "http://127.0.0.1:8765/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

## 4) How the LLM should use the tools

Typical flow:

1. `ptyctl_session` with `action=open` to create a session.
2. `ptyctl_session_io` with `action=read` to get banners/prompts.
3. `ptyctl_session_io` with `action=write` to send `data` or `key`.
4. Continue reading via cursor or tail.

PTY key input (examples): `enter`, `arrow_up`, `ctrl_c`.
Aliases accepted: `ctrl+c`, `ctrl-c`, `arrow-up`, `page-up`.

### LLM Prompt Template (copy/paste)

Use this prompt to guide an LLM to install and connect ptyctl, then open a session and interact:

```
Please install and configure the ptyctl MCP server on this machine.
- If not installed, follow the README install steps.
- Start ptyctl with STDIO transport.
- Add it to the MCP client as `ptyctl-stdio`.

Then open an SSH session to HOST with USERNAME and PASSWORD.
Use expect prompt regex "[#>$]" and interact via ptyctl_session_io:
1) read banner/prompt
2) send username/password if prompted
3) run `show version`
4) read until the prompt returns
Return the final output.
```

## 4.1) Full tool examples (open/read/write/expect/lock)

### Open (SSH) with expect

```json
{
  "action": "open",
  "protocol": "ssh",
  "host": "10.0.0.1",
  "username": "root",
  "auth": {"password": "..."},
  "expect": {"prompt_regex": "[#>$]"}
}
```

### Read banner/prompt (cursor mode)

```json
{
  "action": "read",
  "session_id": "SESSION_ID",
  "cursor": "0",
  "timeout_ms": 2000,
  "max_bytes": 65536,
  "encoding": "utf-8"
}
```

### Write data (send a command)

```json
{
  "action": "write",
  "session_id": "SESSION_ID",
  "data": "show version\\n",
  "encoding": "utf-8"
}
```

### Write key (Ctrl+C)

```json
{
  "action": "write",
  "session_id": "SESSION_ID",
  "key": "ctrl+c"
}
```

### Read until prompt (regex)

```json
{
  "action": "read",
  "session_id": "SESSION_ID",
  "cursor": "CURSOR",
  "timeout_ms": 5000,
  "max_bytes": 65536,
  "until_regex": "[#>$]",
  "include_match": true,
  "encoding": "utf-8"
}
```

### Lock / Unlock (for console sessions)

```json
{
  "action": "lock",
  "session_id": "SESSION_ID",
  "task_id": "TASK_ID",
  "lock_ttl_ms": 60000
}
```

```json
{
  "action": "unlock",
  "session_id": "SESSION_ID",
  "task_id": "TASK_ID"
}
```

### Heartbeat (extend lock)

```json
{
  "action": "heartbeat",
  "session_id": "SESSION_ID",
  "task_id": "TASK_ID",
  "lock_ttl_ms": 60000
}
```

## 5) Monitor LLM actions from the CLI (attach/tail)

ptyctl exposes a local control socket for read-only monitoring by default.

List sessions:

```bash
ptyctl sessions
```

Tail recent output from a session:

```bash
ptyctl tail <SESSION_ID>
```

Attach to a session (stream output as it changes):

```bash
ptyctl attach <SESSION_ID>
```

### Control socket notes

- Default socket path is determined by `XDG_RUNTIME_DIR`, then `/run/user/<uid>`, or `/tmp/ptyctl-<uid>.sock`.
- Override with `--control-socket` or `PTYCTL_CONTROL_SOCKET`.
- If control mode is disabled, these commands will not work.
