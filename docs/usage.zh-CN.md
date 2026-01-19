# ptyctl MCP 使用说明（安装后）

本文聚焦安装完成后的使用方式：如何在 Codex/VSCode 中连接、如何让 LLM 正确调用工具，以及如何在 CLI 中监控 LLM 的操作。

如需安装步骤，请参考 `README.zh-CN.md`。

## 1) 启动 MCP 服务

选择你需要的传输方式：

```bash
ptyctl serve --transport stdio
# 或
ptyctl serve --transport http --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
# 或
ptyctl serve --transport both --http-listen 127.0.0.1:8765 --auth-token YOUR_TOKEN
```

## 2) 在 Codex 中连接

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

## 3) 在 VSCode/Cursor 中连接

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

## 4) LLM 应该如何使用这些工具

典型流程：

1. 用 `ptyctl_session` 的 `action=open` 创建会话。
2. 用 `ptyctl_session_io` 的 `action=read` 读取横幅/提示。
3. 用 `ptyctl_session_io` 的 `action=write` 发送 `data` 或 `key`。
4. 通过游标或 tail 继续读取输出。

PTY 按键示例：`enter`、`arrow_up`、`ctrl_c`。
兼容别名：`ctrl+c`、`ctrl-c`、`arrow-up`、`page-up`。

### LLM 提示模板（可直接复制）

```
请在本机安装并配置 ptyctl MCP 服务：
- 如未安装，请按 README 执行安装。
- 启动 ptyctl（STDIO 传输）。
- 在 MCP 客户端中添加为 `ptyctl-stdio`。

然后通过 ptyctl 打开 SSH 会话（HOST/USERNAME/PASSWORD）。
使用 expect 的 prompt_regex 为 "[#>$]"，并按以下流程交互：
1) 读取横幅/提示
2) 如有登录提示，发送用户名/密码
3) 执行 `show version`
4) 读取直到出现提示符
最后输出结果。
```

## 4.1) 完整工具示例（open/read/write/expect/lock）

### 打开 SSH 会话（含 expect）

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

### 读取横幅/提示（游标模式）

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

### 发送数据（执行命令）

```json
{
  "action": "write",
  "session_id": "SESSION_ID",
  "data": "show version\\n",
  "encoding": "utf-8"
}
```

### 发送按键（Ctrl+C）

```json
{
  "action": "write",
  "session_id": "SESSION_ID",
  "key": "ctrl+c"
}
```

### 读取直到出现提示符（regex）

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

### 加锁/解锁（Console 会话）

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

### 心跳续期（延长锁）

```json
{
  "action": "heartbeat",
  "session_id": "SESSION_ID",
  "task_id": "TASK_ID",
  "lock_ttl_ms": 60000
}
```

## 5) CLI 中监控 LLM 的操作（attach/tail）

ptyctl 默认提供本地控制套接字，便于只读监控。

列出会话：

```bash
ptyctl sessions
```

查看某会话最近输出：

```bash
ptyctl tail <SESSION_ID>
```

实时附着查看输出：

```bash
ptyctl attach <SESSION_ID>
```

### 控制套接字说明

- 默认路径依次为：`XDG_RUNTIME_DIR`、`/run/user/<uid>`、`/tmp/ptyctl-<uid>.sock`。
- 可通过 `--control-socket` 或 `PTYCTL_CONTROL_SOCKET` 覆盖。
- 若控制模式被禁用，上述命令不可用。
