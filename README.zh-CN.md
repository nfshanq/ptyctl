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

### 方式 B：脚本安装（推荐）

脚本会安装二进制，并根据你选择的 agent/transport 输出下一步配置命令。

#### B1. STDIO + Codex（默认）

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash
```

添加到 Codex（stdio）：

```bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

#### B2. STDIO + VSCode/Cursor

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --agent cursor
```

在 VSCode/Cursor 设置里添加（`mcpServers`）：

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

启动服务：

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

添加到 Codex（HTTP）：

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

启动服务：

```bash
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

在 VSCode/Cursor 设置里添加（`mcpServers`）：

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

### 方式 C：手动下载安装（不使用脚本）

1) 选择正确的系统/架构对应的资产：

```bash
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$OS-$ARCH" in
  linux-x86_64) ASSET=ptyctl-linux-amd64.tar.gz ;;
  darwin-arm64) ASSET=ptyctl-macos-arm64.tar.gz ;;
  *) echo "不支持的系统/架构: $OS-$ARCH" && exit 1 ;;
esac
```

2) 下载并安装：

```bash
curl -L -o /tmp/$ASSET https://github.com/nfshanq/ptyctl/releases/latest/download/$ASSET
tar -xzf /tmp/$ASSET -C /tmp
BIN_NAME=${ASSET%.tar.gz}
sudo install -m 0755 /tmp/$BIN_NAME /usr/local/bin/ptyctl
```

macOS 提示（通过浏览器/Finder 手动下载时）：

```bash
sudo xattr -d com.apple.quarantine /usr/local/bin/ptyctl
```

## 启动 MCP 服务（参考）

以下示例直接调用已安装的 `ptyctl`。

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
