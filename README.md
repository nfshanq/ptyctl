# ptyctl

English | [中文](README.zh-CN.md)

Rust-based MCP server for remote interactive control over SSH and Telnet. It supports both STDIO and HTTP transports, session management, and a cursor-based output buffer for reliable interactive reads.

## Features

- MCP tools: `ptyctl_session`, `ptyctl_session_exec`, `ptyctl_session_io`, `ptyctl_session_config`.
- Protocols: SSH and Telnet (in-process Telnet protocol handling).
- Transports: STDIO, HTTP (JSON-RPC + SSE).
- Output cursors: independent readers can follow a session buffer without interfering.
- Exit code extraction: default marker + ASCII fallback when control characters are stripped.
- PTY key inputs via `ptyctl_session_io` write `key` (e.g. `enter`, `arrow_up`, `ctrl_c`; aliases like `ctrl+c`, `ctrl-c`, `arrow-up`, `page-up` are accepted).

## Docs

- Usage guide (EN): `docs/usage.md`
- 使用说明（中文）：`docs/usage.zh-CN.md`

## Build and Test

```bash
cargo build --release
cargo test
```

## Install

### LLM Quick Install (copy/paste)

If you want an LLM to install and wire up ptyctl in one shot, use these commands.

STDIO (Codex):

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

HTTP (Codex):

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --transport http
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
export PTYCTL_AUTH_TOKEN=YOUR_TOKEN
codex mcp add ptyctl-http \
  --url http://127.0.0.1:8765/mcp \
  --bearer-token-env-var PTYCTL_AUTH_TOKEN
```

### Option A: Build from source (local)

```bash
cargo build --release
```

The binary will be at `target/release/ptyctl`. To install it into `/usr/local/bin`:

```bash
sudo install -m 0755 target/release/ptyctl /usr/local/bin/ptyctl
```

### Option B: Install via script (recommended)

The script installs the binary and prints next steps based on the agent/transport you choose.

#### B1. STDIO + Codex (default)

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash
```

Add to Codex (stdio):

```bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

#### B2. STDIO + VSCode/Cursor

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --agent cursor
```

Add to VSCode/Cursor settings (`mcpServers`):

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

#### B3. HTTP + Codex

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --transport http
```

Start the server:

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

Add to Codex (HTTP):

```bash
export PTYCTL_AUTH_TOKEN=YOUR_TOKEN
codex mcp add ptyctl-http \
  --url http://127.0.0.1:8765/mcp \
  --bearer-token-env-var PTYCTL_AUTH_TOKEN
```

#### B4. HTTP + VSCode/Cursor

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --transport http --agent cursor
```

Start the server:

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

Add to VSCode/Cursor settings (`mcpServers`):

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

### Option C: Manual download (no script)

1) Open the latest release page and download the correct asset for your OS/arch:

```bash
https://github.com/nfshanq/ptyctl/releases/latest
```

Assets:

- macOS (Apple Silicon / arm64): `ptyctl-macos-arm64.tar.gz`
- Linux (x86_64): `ptyctl-linux-amd64.tar.gz`

If your OS/arch is not listed, build from source (Option A).

2) Extract the archive:

```bash
tar -xzf ptyctl-macos-arm64.tar.gz
# or
tar -xzf ptyctl-linux-amd64.tar.gz
```

3) Install the binary into `/usr/local/bin`:

```bash
sudo install -m 0755 ptyctl-macos-arm64 /usr/local/bin/ptyctl
# or
sudo install -m 0755 ptyctl-linux-amd64 /usr/local/bin/ptyctl
```

macOS note (manual download via browser/Finder):

```bash
sudo xattr -d com.apple.quarantine /usr/local/bin/ptyctl
```

## Run the MCP Server (reference)

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
