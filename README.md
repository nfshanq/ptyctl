# ptyctl

English | [中文](README.zh-CN.md)

Rust-based MCP server for remote interactive control over SSH and Telnet. It supports both STDIO and HTTP transports, session management, and a cursor-based output buffer for reliable interactive reads.

## Features

- MCP tools: `ptyctl_session`, `ptyctl_session_exec`, `ptyctl_session_io`, `ptyctl_session_config`.
- Protocols: SSH and Telnet (in-process Telnet protocol handling).
- Transports: STDIO, HTTP (JSON-RPC + SSE).
- Output cursors: independent readers can follow a session buffer without interfering.
- Exit code extraction: default marker + ASCII fallback when control characters are stripped.

## Build and Test

```bash
cargo build --release
cargo test
```

## Install

### Option A: Build from source (local)

```bash
cargo build --release
```

The binary will be at `target/release/ptyctl`. To install it into `/usr/local/bin`:

```bash
sudo install -m 0755 target/release/ptyctl /usr/local/bin/ptyctl
```

### Option B: Download prebuilt binaries (GitHub Releases)

One-liner install (Linux/macOS, auto-detect):

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash
```

Manual download (if you prefer):

```bash
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$OS-$ARCH" in
  linux-x86_64) ASSET=ptyctl-linux-amd64.tar.gz ;;
  darwin-arm64) ASSET=ptyctl-macos-arm64.tar.gz ;;
  *) echo "Unsupported OS/arch: $OS-$ARCH" && exit 1 ;;
esac
curl -L -o /tmp/$ASSET https://github.com/nfshanq/ptyctl/releases/latest/download/$ASSET
tar -xzf /tmp/$ASSET -C /tmp
BIN_NAME=${ASSET%.tar.gz}
sudo install -m 0755 /tmp/$BIN_NAME /usr/local/bin/ptyctl
```

## Run the MCP Server (installed binary)

### STDIO mode (for local MCP clients)

```bash
ptyctl serve --transport stdio
```

### HTTP mode (for local/LAN usage)

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

HTTP endpoint:
- Streamable HTTP: `http://127.0.0.1:8765/mcp` (POST JSON-RPC, GET SSE)

### Both modes

```bash
ptyctl serve --transport both --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

### Optional control socket

For local read-only monitoring (e.g., `ptyctl sessions`, `ptyctl tail`, `ptyctl attach`):

```bash
ptyctl serve --transport stdio --control-socket /tmp/ptyctl.sock --control-mode readonly
```

## Cursor and Codex MCP Setup

### Cursor (MCP servers)

Open Cursor settings and add an MCP server. Example config:

STDIO transport:

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

HTTP transport:

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

Notes:
- Use the full path to the `ptyctl` binary.
- If you build release: `cargo build --release` then use `target/release/ptyctl`.
- The HTTP server uses MCP streamable HTTP at `/mcp` (POST JSON-RPC, GET SSE).

### Codex CLI

Add `ptyctl` as an MCP server in your Codex configuration. The exact file location may vary by installation; common locations are:
- `~/.codex/mcp.json`
- `~/.config/codex/mcp.json`

Add via `codex mcp add` (STDIO):

```bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

STDIO example:

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

HTTP example:

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

## Quick Tool Examples

Open an SSH session (MCP tools/call):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
      "name": "ptyctl_session",
      "arguments": {
        "action": "open",
        "protocol": "ssh",
        "host": "example.com",
        "port": 22,
        "pty": { "enabled": true, "cols": 120, "rows": 40, "term": "xterm-256color" }
      }
    }
}
```

Execute a command (exit code via markers):

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "ptyctl_session_exec",
    "arguments": {
      "session_id": "SESSION_ID",
      "cmd": "echo hello",
      "timeout_ms": 20000
    }
  }
}
```

Read output with cursor:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
      "name": "ptyctl_session_io",
      "arguments": {
        "action": "read",
        "session_id": "SESSION_ID",
        "cursor": "0",
        "timeout_ms": 2000,
        "max_bytes": 65536,
        "encoding": "utf-8"
      }
    }
}
```
