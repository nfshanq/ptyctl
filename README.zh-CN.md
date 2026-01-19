# ptyctl

[English](README.md) | 中文

基于 Rust 的 MCP 服务，可通过 SSH/Telnet 远程交互式控制。支持 STDIO 与 HTTP 传输，包含会话管理与游标式输出缓冲，保证交互读取可靠。

## 特性

- MCP 工具：`ptyctl_session`、`ptyctl_session_exec`、`ptyctl_session_io`、`ptyctl_session_config`。
- 协议：SSH 与 Telnet（内置 Telnet 协议处理）。
- 传输：STDIO、HTTP（JSON-RPC + SSE）。
- 输出游标：多个读取方可独立跟随会话缓冲互不干扰。
- 退出码提取：默认 marker + ASCII 兜底（适用于控制字符被剥离的情况）。
- 通过 `ptyctl_session_io` 的 write `key` 支持 PTY 按键（如 `enter`、`arrow_up`、`ctrl_c`；兼容 `ctrl+c`、`ctrl-c`、`arrow-up`、`page-up` 等别名）。

## 文档

- Usage guide (EN)：`docs/usage.md`
- 使用说明（中文）：`docs/usage.zh-CN.md`

## 构建与测试

```bash
cargo build --release
cargo test
```

## 安装

### LLM 快速安装（可直接复制执行）

STDIO（Codex）：

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash
codex mcp add ptyctl-stdio \
  --env PTYCTL_LOG_LEVEL=info \
  -- /usr/local/bin/ptyctl serve --transport stdio
```

HTTP（Codex）：

```bash
curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --transport http
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
export PTYCTL_AUTH_TOKEN=YOUR_TOKEN
codex mcp add ptyctl-http \
  --url http://127.0.0.1:8765/mcp \
  --bearer-token-env-var PTYCTL_AUTH_TOKEN
```

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

1) 打开最新 Release 页面并下载对应系统/架构的资产：

```bash
https://github.com/nfshanq/ptyctl/releases/latest
```

资产对应关系：

- macOS（Apple Silicon / arm64）：`ptyctl-macos-arm64.tar.gz`
- Linux（x86_64）：`ptyctl-linux-amd64.tar.gz`

如果你的系统/架构不在列表中，请使用方式 A 从源码编译。

2) 解压压缩包：

```bash
tar -xzf ptyctl-macos-arm64.tar.gz
# 或
tar -xzf ptyctl-linux-amd64.tar.gz
```

3) 将二进制安装到 `/usr/local/bin`：

```bash
sudo install -m 0755 ptyctl-macos-arm64 /usr/local/bin/ptyctl
# 或
sudo install -m 0755 ptyctl-linux-amd64 /usr/local/bin/ptyctl
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
