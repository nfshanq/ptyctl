# ptyctl

[English](README.md) | 中文

基于 Rust 的 MCP 服务，可通过 SSH/Telnet 远程交互式控制。支持 STDIO 与 HTTP 传输，包含会话管理与游标式输出缓冲，保证交互读取可靠。

## 特性

- MCP 工具：`ptyctl_session`、`ptyctl_session_exec`、`ptyctl_session_io`、`ptyctl_session_config`。
- 协议：SSH 与 Telnet（内置 Telnet 协议处理）。
- 传输：STDIO、HTTP（JSON-RPC + SSE）。
- 输出游标：多个读取方可独立跟随会话缓冲互不干扰。
- 退出码提取：默认 marker + ASCII 兜底（适用于控制字符被剥离的情况）。

## 构建与测试

```bash
cargo build --release
cargo test
```

## 安装

### 方式 A：从源码编译

```bash
cargo build --release
```

编译后的可执行文件位于 `target/release/ptyctl`。安装到 `/usr/local/bin`：

```bash
sudo install -m 0755 target/release/ptyctl /usr/local/bin/ptyctl
```

### 方式 B：直接下载 GitHub Releases 二进制

一键安装（自动识别 Linux/macOS）：

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/pytctl/main/install.sh | bash
```

手动下载（如需）：

```bash
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$OS-$ARCH" in
  linux-x86_64) ASSET=ptyctl-linux-amd64.tar.gz ;;
  darwin-arm64) ASSET=ptyctl-macos-arm64.tar.gz ;;
  *) echo "不支持的系统/架构: $OS-$ARCH" && exit 1 ;;
esac
curl -L -o /tmp/$ASSET https://github.com/nfshanq/pytctl/releases/latest/download/$ASSET
tar -xzf /tmp/$ASSET -C /tmp
BIN_NAME=${ASSET%.tar.gz}
sudo install -m 0755 /tmp/$BIN_NAME /usr/local/bin/ptyctl
```

## 启动 MCP 服务（安装后的二进制）

假设使用 `target/release/ptyctl` 作为可执行文件。

### STDIO 模式（本地 MCP 客户端）

```bash
ptyctl serve --transport stdio
```

### HTTP 模式（本地/LAN）

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

HTTP 端点：
- Streamable HTTP: `http://127.0.0.1:8765/mcp`（POST JSON-RPC，GET SSE）

### 同时开启 STDIO + HTTP

```bash
ptyctl serve --transport both --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

### 可选控制套接字

用于本地只读监控（如 `ptyctl sessions`、`ptyctl tail`、`ptyctl attach`）：

```bash
ptyctl serve --transport stdio --control-socket /tmp/ptyctl.sock --control-mode readonly
```

## Cursor 与 Codex MCP 配置

### Cursor（MCP 服务器）

在 Cursor 设置中添加 MCP 服务器，示例配置如下。

STDIO 模式：

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

HTTP 模式：

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

说明：
- `command` 请使用 `ptyctl` 的绝对路径。
- 如需 release 版本：`cargo build --release`，再使用 `target/release/ptyctl`。
- HTTP 服务器使用 `/mcp` 的 MCP streamable HTTP（POST JSON-RPC，GET SSE）。

### Codex CLI

在 Codex 的 MCP 配置文件中添加 `ptyctl`。配置文件位置因环境不同可能有所差异，常见路径：
- `~/.codex/mcp.json`
- `~/.config/codex/mcp.json`

使用 `codex mcp add`（STDIO）：

```bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

STDIO 示例：

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

HTTP 示例：

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

Open an SSH session（MCP tools/call）:

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
