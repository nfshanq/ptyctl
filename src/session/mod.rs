mod buffer;
mod ssh;
mod telnet;

use crate::config::{SessionConfig, SshConfig, TelnetLineEnding};
use crate::error::{ApiError, ErrorCode, PtyResult};
use async_trait::async_trait;
use buffer::{BufferSlice, OutputBuffer, TailSlice};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ssh::{SshBackend, SshConnectParams};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use telnet::TelnetBackend;
use tokio::sync::{Notify, RwLock};
use tokio::time::{Instant, sleep};
use uuid::Uuid;

pub use buffer::{BufferSlice as OutputBufferSlice, TailSlice as OutputTailSlice};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ssh,
    Telnet,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SessionType {
    #[default]
    Normal,
    Console,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub enum Encoding {
    #[serde(rename = "utf-8", alias = "utf8", alias = "utf_8")]
    #[default]
    Utf8,
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyOptions {
    #[schemars(description = "Enable PTY allocation.")]
    pub enabled: bool,
    #[schemars(description = "Terminal columns.")]
    pub cols: u16,
    #[schemars(description = "Terminal rows.")]
    pub rows: u16,
    #[schemars(description = "Terminal type, e.g. xterm-256color.")]
    pub term: String,
}

impl Default for PtyOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            cols: 120,
            rows: 40,
            term: "xterm-256color".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct Timeouts {
    pub connect_timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ExpectConfig {
    #[schemars(description = "Regex for shell prompt detection.")]
    pub prompt_regex: Option<String>,
    #[schemars(description = "Regexes for pager prompts (e.g. --More--).")]
    pub pager_regexes: Option<Vec<String>>,
    #[schemars(description = "Regexes for error patterns to surface.")]
    pub error_regexes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshAuth {
    #[schemars(description = "Authentication method hint (optional).")]
    pub method: Option<String>,
    #[schemars(description = "Password for password-based authentication.")]
    pub password: Option<String>,
    #[schemars(description = "Private key in PEM format.")]
    pub private_key_pem: Option<String>,
    #[schemars(description = "Passphrase for the private key, if needed.")]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct SshOptions {
    pub host_key_policy: Option<String>,
    pub known_hosts_path: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub use_openssh_config: Option<bool>,
    pub config_path: Option<String>,
    pub extra_args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenRequest {
    pub protocol: Protocol,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth: Option<SshAuth>,
    pub pty: Option<PtyOptions>,
    pub timeouts: Option<Timeouts>,
    pub ssh_options: Option<SshOptions>,
    #[schemars(description = "Expect configuration object.")]
    pub expect: Option<ExpectConfig>,
    pub session_type: Option<SessionType>,
    pub device_id: Option<String>,
    pub acquire_lock: Option<bool>,
    pub lock_ttl_ms: Option<u64>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOpenResponse {
    pub session_id: String,
    pub protocol: Protocol,
    pub pty_enabled: bool,
    pub server_banner: Option<String>,
    pub security_warning: Option<String>,
    pub lock_acquired: Option<bool>,
    pub existing_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionWriteRequest {
    pub session_id: String,
    pub data: String,
    pub encoding: Option<Encoding>,
    pub sensitive: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionWriteResponse {
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionSendRequest {
    pub session_id: String,
    pub key: SessionKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSendResponse {
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionReadRequest {
    pub session_id: String,
    pub cursor: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_bytes: Option<usize>,
    pub until_regex: Option<String>,
    pub include_match: Option<bool>,
    pub until_idle_ms: Option<u64>,
    pub encoding: Option<Encoding>,
    pub input_hints: Option<InputHints>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InputHints {
    pub wait_for_regexes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadResponse {
    pub chunk: String,
    pub encoding: Encoding,
    pub next_cursor: String,
    pub buffer_start_cursor: String,
    pub buffer_end_cursor: String,
    pub matched: bool,
    pub idle_reached: bool,
    pub timed_out: bool,
    pub eof: bool,
    pub waiting_for_input: Option<bool>,
    pub truncated: bool,
    pub dropped_bytes: u64,
    pub buffered_bytes: usize,
    pub buffer_limit_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionResizeRequest {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResizeResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCloseRequest {
    pub session_id: String,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCloseResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListEntry {
    pub session_id: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub created_at: u64,
    pub last_activity_at: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub state: SessionState,
    pub session_type: SessionType,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionExpectRequest {
    pub session_id: String,
    pub expect: ExpectConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExpectResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionTailRequest {
    pub session_id: String,
    pub max_bytes: Option<usize>,
    pub max_lines: Option<usize>,
    pub encoding: Option<Encoding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTailResponse {
    pub tail: String,
    pub encoding: Encoding,
    pub tail_start_cursor: String,
    pub next_cursor: String,
    pub buffer_start_cursor: String,
    pub buffer_end_cursor: String,
    pub truncated: bool,
    pub buffered_bytes: usize,
    pub buffer_limit_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionExecRequest {
    pub session_id: String,
    pub cmd: String,
    pub timeout_ms: Option<u64>,
    pub until_idle_ms: Option<u64>,
    pub rc_mode: Option<RcMode>,
    pub expect: Option<ExpectConfig>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RcMode {
    pub enabled: Option<bool>,
    pub marker_prefix: Option<String>,
    pub marker_suffix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub exit_code_reason: Option<String>,
    pub done_reason: String,
    pub prompt_detected: Option<bool>,
    pub error_hints: Option<Vec<String>>,
    pub timed_out: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionAction {
    Open,
    Close,
    List,
    Lock,
    Unlock,
    Heartbeat,
    Status,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IoAction {
    Write,
    Read,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReadMode {
    Cursor,
    Tail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConfigAction {
    Resize,
    Expect,
    Get,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    pub supports_split_stdout_stderr: bool,
    pub supports_exit_code: bool,
    pub supports_resize: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRequest {
    #[schemars(description = "Session action: open/close/list/lock/unlock/heartbeat/status.")]
    pub action: SessionAction,
    #[schemars(
        description = "Connection protocol (required for action=open): \"ssh\" or \"telnet\"."
    )]
    pub protocol: Option<Protocol>,
    #[schemars(description = "Remote host (required for action=open).")]
    pub host: Option<String>,
    #[schemars(description = "Remote port (optional; defaults to 22 for ssh, 23 for telnet).")]
    pub port: Option<u16>,
    #[schemars(description = "Username for authentication (optional for telnet/ssh).")]
    pub username: Option<String>,
    #[schemars(description = "Authentication object. For password auth: {\"password\":\"...\"}.")]
    pub auth: Option<SshAuth>,
    #[schemars(
        description = "PTY options. Omit to use defaults (enabled=true, cols=120, rows=40, term=xterm-256color)."
    )]
    pub pty: Option<PtyOptions>,
    pub timeouts: Option<Timeouts>,
    pub ssh_options: Option<SshOptions>,
    pub expect: Option<ExpectConfig>,
    pub session_type: Option<SessionType>,
    pub device_id: Option<String>,
    pub acquire_lock: Option<bool>,
    pub lock_ttl_ms: Option<u64>,
    #[schemars(description = "Existing session id (required for non-open actions).")]
    pub session_id: Option<String>,
    pub force: Option<bool>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub action: SessionAction,
    pub success: bool,
    pub session_id: Option<String>,
    pub protocol: Option<Protocol>,
    pub pty_enabled: Option<bool>,
    pub security_warning: Option<String>,
    pub lock_acquired: Option<bool>,
    pub existing_session_id: Option<String>,
    pub sessions: Option<Vec<SessionListEntry>>,
    pub capabilities: Option<Capabilities>,
    pub lock_holder: Option<String>,
    pub lock_expires_at: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionIoRequest {
    pub session_id: String,
    pub action: IoAction,
    pub data: Option<String>,
    pub key: Option<SessionKey>,
    pub encoding: Option<Encoding>,
    pub sensitive: Option<bool>,
    pub mode: Option<ReadMode>,
    pub cursor: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_bytes: Option<usize>,
    pub max_lines: Option<usize>,
    pub until_regex: Option<String>,
    pub include_match: Option<bool>,
    pub until_idle_ms: Option<u64>,
    pub input_hints: Option<InputHints>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIoResponse {
    pub action: IoAction,
    pub bytes_written: Option<usize>,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfigRequest {
    pub session_id: String,
    pub action: ConfigAction,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub expect: Option<ExpectConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigResponse {
    pub success: bool,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub expect: Option<ExpectConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionKey {
    Enter,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    #[serde(alias = "ctrl+c", alias = "ctrl-c")]
    CtrlC,
    #[serde(alias = "ctrl+d", alias = "ctrl-d")]
    CtrlD,
    #[serde(alias = "ctrl+z", alias = "ctrl-z")]
    CtrlZ,
    #[serde(
        alias = "ctrl+backslash",
        alias = "ctrl-backslash",
        alias = "ctrl+\\",
        alias = "ctrl-\\"
    )]
    CtrlBackslash,
    #[serde(alias = "ctrl+a", alias = "ctrl-a")]
    CtrlA,
    #[serde(alias = "ctrl+e", alias = "ctrl-e")]
    CtrlE,
    #[serde(alias = "ctrl+k", alias = "ctrl-k")]
    CtrlK,
    #[serde(alias = "ctrl+u", alias = "ctrl-u")]
    CtrlU,
    #[serde(alias = "ctrl+l", alias = "ctrl-l")]
    CtrlL,
    Esc,
    #[serde(alias = "arrow-up")]
    ArrowUp,
    #[serde(alias = "arrow-down")]
    ArrowDown,
    #[serde(alias = "arrow-left")]
    ArrowLeft,
    #[serde(alias = "arrow-right")]
    ArrowRight,
    #[serde(alias = "page-up")]
    PageUp,
    #[serde(alias = "page-down")]
    PageDown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Open,
    Closing,
    Closed,
    Error,
}

#[async_trait]
pub trait SessionBackend: Send + Sync {
    async fn write(&self, data: &[u8]) -> PtyResult<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> PtyResult<()>;
    async fn close(&self, force: bool) -> PtyResult<()>;
    fn is_eof(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct LockInfo {
    pub task_id: String,
    pub acquired_at: u64,
    pub expires_at: u64,
    pub heartbeat_interval_ms: u64,
}

pub struct Session {
    pub id: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub session_type: SessionType,
    pub device_id: Option<String>,
    buffer: Arc<Mutex<OutputBuffer>>,
    notify: Arc<Notify>,
    backend: Box<dyn SessionBackend>,
    expect: Arc<RwLock<ExpectConfig>>,
    state: AtomicU64,
    created_at: u64,
    last_activity: Arc<AtomicU64>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    idle_timeout_ms: u64,
    telnet_line_ending: TelnetLineEnding,
    record_tx_events: bool,
    pty_enabled: bool,
    pty_cols: AtomicU64,
    pty_rows: AtomicU64,
    lock_holder: RwLock<Option<LockInfo>>,
}

#[derive(Clone)]
pub struct OutputHandle {
    pub session_id: String,
    buffer: Arc<Mutex<OutputBuffer>>,
    notify: Arc<Notify>,
    bytes_in: Arc<AtomicU64>,
    last_activity: Arc<AtomicU64>,
}

impl OutputHandle {
    pub fn append_output(&self, bytes: &[u8]) {
        let mut buffer = self.buffer.lock().expect("output buffer mutex poisoned");
        let dropped = buffer.append(bytes);
        if dropped > 0 {
            tracing::warn!(
                session_id = %self.session_id,
                dropped_bytes = dropped,
                "Output buffer overflowed; oldest data dropped"
            );
        }
        self.bytes_in
            .fetch_add(bytes.len() as u64, Ordering::SeqCst);
        self.last_activity.store(now_ms(), Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

struct SessionInit {
    id: String,
    protocol: Protocol,
    host: String,
    port: u16,
    session_type: SessionType,
    device_id: Option<String>,
    backend: Box<dyn SessionBackend>,
    buffer: Arc<Mutex<OutputBuffer>>,
    notify: Arc<Notify>,
    last_activity: Arc<AtomicU64>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    expect: ExpectConfig,
    pty: PtyOptions,
    idle_timeout_ms: u64,
    telnet_line_ending: TelnetLineEnding,
    record_tx_events: bool,
}

impl Session {
    fn new(init: SessionInit) -> Self {
        let now = now_ms();
        Self {
            id: init.id,
            protocol: init.protocol,
            host: init.host,
            port: init.port,
            session_type: init.session_type,
            device_id: init.device_id,
            buffer: init.buffer,
            notify: init.notify,
            backend: init.backend,
            expect: Arc::new(RwLock::new(init.expect)),
            state: AtomicU64::new(SessionState::Open as u64),
            created_at: now,
            last_activity: init.last_activity,
            bytes_in: init.bytes_in,
            bytes_out: init.bytes_out,
            idle_timeout_ms: init.idle_timeout_ms,
            telnet_line_ending: init.telnet_line_ending,
            record_tx_events: init.record_tx_events,
            pty_enabled: init.pty.enabled,
            pty_cols: AtomicU64::new(init.pty.cols as u64),
            pty_rows: AtomicU64::new(init.pty.rows as u64),
            lock_holder: RwLock::new(None),
        }
    }

    pub fn state(&self) -> SessionState {
        match self.state.load(Ordering::SeqCst) {
            x if x == SessionState::Open as u64 => SessionState::Open,
            x if x == SessionState::Closing as u64 => SessionState::Closing,
            x if x == SessionState::Closed as u64 => SessionState::Closed,
            _ => SessionState::Error,
        }
    }

    pub fn set_state(&self, state: SessionState) {
        self.state.store(state as u64, Ordering::SeqCst);
    }

    pub fn metrics(&self) -> (u64, u64, u64, u64) {
        (
            self.created_at,
            self.last_activity.load(Ordering::SeqCst),
            self.bytes_in.load(Ordering::SeqCst),
            self.bytes_out.load(Ordering::SeqCst),
        )
    }

    pub fn buffer_snapshot(&self) -> BufferSlice {
        let buffer = self.lock_buffer();
        buffer.slice_from(buffer.buffer_start(), buffer.buffered_bytes())
    }

    pub fn tail(&self, max_bytes: usize, max_lines: Option<usize>) -> TailSlice {
        let buffer = self.lock_buffer();
        buffer.tail(max_bytes, max_lines)
    }

    pub fn buffer_end_cursor(&self) -> u64 {
        let buffer = self.lock_buffer();
        buffer.buffer_end()
    }

    pub fn buffer_start_cursor(&self) -> u64 {
        let buffer = self.lock_buffer();
        buffer.buffer_start()
    }

    pub fn notify_new_data(&self) {
        self.notify.notify_waiters();
    }

    pub fn append_output(&self, bytes: &[u8]) {
        let mut buffer = self.lock_buffer_mut();
        let dropped = buffer.append(bytes);
        if dropped > 0 {
            tracing::warn!(
                session_id = %self.id,
                dropped_bytes = dropped,
                "Output buffer overflowed; oldest data dropped"
            );
        }
        self.bytes_in
            .fetch_add(bytes.len() as u64, Ordering::SeqCst);
        self.touch();
    }

    pub fn touch(&self) {
        self.last_activity.store(now_ms(), Ordering::SeqCst);
    }

    fn lock_buffer(&self) -> std::sync::MutexGuard<'_, OutputBuffer> {
        self.buffer.lock().expect("buffer mutex poisoned")
    }

    fn lock_buffer_mut(&self) -> std::sync::MutexGuard<'_, OutputBuffer> {
        self.buffer.lock().expect("buffer mutex poisoned")
    }

    pub async fn write(&self, data: &[u8], sensitive: bool) -> PtyResult<usize> {
        let mut payload = data.to_vec();
        if self.protocol == Protocol::Telnet {
            payload = normalize_telnet_line_endings(&payload, self.telnet_line_ending.clone());
        }
        let written = self.backend.write(&payload).await?;
        self.bytes_out.fetch_add(written as u64, Ordering::SeqCst);
        self.touch();
        if self.record_tx_events && sensitive {
            tracing::info!(session_id = %self.id, "Sensitive write occurred");
        }
        Ok(written)
    }

    pub async fn send_key(&self, key: SessionKey) -> PtyResult<usize> {
        let bytes = key_bytes(self.protocol, key)?;
        self.write(&bytes, false).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> PtyResult<()> {
        self.backend.resize(cols, rows).await?;
        self.pty_cols.store(cols as u64, Ordering::SeqCst);
        self.pty_rows.store(rows as u64, Ordering::SeqCst);
        Ok(())
    }

    pub async fn close(&self, force: bool) -> PtyResult<()> {
        self.set_state(SessionState::Closing);
        let result = self.backend.close(force).await;
        self.set_state(SessionState::Closed);
        result
    }

    pub fn is_eof(&self) -> bool {
        self.backend.is_eof()
    }

    pub async fn set_expect(&self, expect: ExpectConfig) {
        let mut guard = self.expect.write().await;
        *guard = expect;
    }

    pub async fn expect(&self) -> ExpectConfig {
        self.expect.read().await.clone()
    }

    pub fn pty_enabled(&self) -> bool {
        self.pty_enabled
    }

    pub fn pty_size(&self) -> (u16, u16) {
        (
            self.pty_cols.load(Ordering::SeqCst) as u16,
            self.pty_rows.load(Ordering::SeqCst) as u16,
        )
    }

    pub fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    pub async fn ensure_write_access(&self, task_id: Option<&str>) -> PtyResult<()> {
        if let Some(lock) = self.lock_status().await {
            let task_id = task_id.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidArgument,
                    "task_id is required for locked sessions",
                )
            })?;
            if lock.task_id != task_id {
                return Err(ApiError::new(
                    ErrorCode::InvalidArgument,
                    format!("Session locked by task {}", lock.task_id),
                )
                .into());
            }
        } else if self.session_type == SessionType::Console {
            return Err(ApiError::new(
                ErrorCode::InvalidArgument,
                "Console sessions require a lock for write access",
            )
            .into());
        }
        Ok(())
    }

    pub async fn lock_status(&self) -> Option<LockInfo> {
        let now = now_ms();
        let mut guard = self.lock_holder.write().await;
        Self::prune_expired_lock(&mut guard, now);
        guard.clone()
    }

    pub async fn lock(&self, task_id: &str, ttl_ms: u64) -> PtyResult<LockInfo> {
        let now = now_ms();
        let ttl_ms = ttl_ms.max(1);
        let mut guard = self.lock_holder.write().await;
        Self::prune_expired_lock(&mut guard, now);
        match guard.as_mut() {
            Some(info) if info.task_id == task_id => {
                info.expires_at = now.saturating_add(ttl_ms);
                info.heartbeat_interval_ms = ttl_ms;
                Ok(info.clone())
            }
            Some(info) => Err(ApiError::new(
                ErrorCode::InvalidArgument,
                format!("Session locked by task {}", info.task_id),
            )
            .into()),
            None => {
                let info = LockInfo {
                    task_id: task_id.to_string(),
                    acquired_at: now,
                    expires_at: now.saturating_add(ttl_ms),
                    heartbeat_interval_ms: ttl_ms,
                };
                *guard = Some(info.clone());
                Ok(info)
            }
        }
    }

    pub async fn heartbeat(&self, task_id: &str, ttl_ms: Option<u64>) -> PtyResult<LockInfo> {
        let now = now_ms();
        let mut guard = self.lock_holder.write().await;
        Self::prune_expired_lock(&mut guard, now);
        match guard.as_mut() {
            Some(info) if info.task_id == task_id => {
                let ttl_ms = ttl_ms.unwrap_or(info.heartbeat_interval_ms).max(1);
                info.heartbeat_interval_ms = ttl_ms;
                info.expires_at = now.saturating_add(ttl_ms);
                Ok(info.clone())
            }
            Some(info) => Err(ApiError::new(
                ErrorCode::InvalidArgument,
                format!("Session locked by task {}", info.task_id),
            )
            .into()),
            None => Err(ApiError::new(ErrorCode::InvalidArgument, "Session is not locked").into()),
        }
    }

    pub async fn unlock(&self, task_id: &str) -> PtyResult<()> {
        let now = now_ms();
        let mut guard = self.lock_holder.write().await;
        Self::prune_expired_lock(&mut guard, now);
        match guard.as_ref() {
            Some(info) if info.task_id == task_id => {
                *guard = None;
                Ok(())
            }
            Some(info) => Err(ApiError::new(
                ErrorCode::InvalidArgument,
                format!("Session locked by task {}", info.task_id),
            )
            .into()),
            None => Err(ApiError::new(ErrorCode::InvalidArgument, "Session is not locked").into()),
        }
    }

    fn prune_expired_lock(lock: &mut Option<LockInfo>, now: u64) {
        if let Some(info) = lock.as_ref() {
            if info.expires_at <= now {
                *lock = None;
            }
        }
    }
}

pub struct SessionManager {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    console_sessions: RwLock<HashMap<String, String>>,
    session_config: SessionConfig,
    ssh_config: SshConfig,
    telnet_line_ending: TelnetLineEnding,
    cleanup_running: AtomicBool,
}

impl SessionManager {
    pub fn new(
        session_config: SessionConfig,
        ssh_config: SshConfig,
        telnet_line_ending: TelnetLineEnding,
    ) -> Arc<Self> {
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            console_sessions: RwLock::new(HashMap::new()),
            session_config,
            ssh_config,
            telnet_line_ending,
            cleanup_running: AtomicBool::new(false),
        })
    }

    pub async fn open_session(
        self: &Arc<Self>,
        request: SessionOpenRequest,
    ) -> PtyResult<SessionOpenResponse> {
        let session_type = request.session_type.unwrap_or_default();
        let device_id = request.device_id.clone();
        if session_type == SessionType::Console {
            let device_id = device_id.clone().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidArgument,
                    "device_id is required for console sessions",
                )
            })?;
            if let Some(existing_id) = self.console_sessions.read().await.get(&device_id).cloned() {
                if let Some(existing_session) =
                    self.sessions.read().await.get(&existing_id).cloned()
                {
                    return Ok(SessionOpenResponse {
                        session_id: existing_id.clone(),
                        protocol: existing_session.protocol,
                        pty_enabled: existing_session.pty_enabled(),
                        server_banner: None,
                        security_warning: if existing_session.protocol == Protocol::Telnet {
                            Some(
                                "Telnet is cleartext; credentials and data are not encrypted."
                                    .to_string(),
                            )
                        } else {
                            None
                        },
                        lock_acquired: None,
                        existing_session_id: Some(existing_id),
                    });
                }
                self.console_sessions.write().await.remove(&device_id);
            }
        }

        if self.sessions.read().await.len() >= self.session_config.max_sessions {
            return Err(ApiError::new(ErrorCode::InvalidArgument, "Too many sessions").into());
        }

        let pty = request.pty.unwrap_or_default();
        let port = match request.protocol {
            Protocol::Ssh => request.port.unwrap_or(22),
            Protocol::Telnet => request.port.unwrap_or(23),
        };
        let id = Uuid::new_v4().to_string();
        let buffer = Arc::new(Mutex::new(OutputBuffer::new(
            self.session_config.output_buffer_max_bytes,
            self.session_config.output_buffer_max_lines,
        )));
        let notify = Arc::new(Notify::new());
        let last_activity = Arc::new(AtomicU64::new(now_ms()));
        let bytes_in = Arc::new(AtomicU64::new(0));
        let bytes_out = Arc::new(AtomicU64::new(0));
        let output = OutputHandle {
            session_id: id.clone(),
            buffer: buffer.clone(),
            notify: notify.clone(),
            bytes_in: bytes_in.clone(),
            last_activity: last_activity.clone(),
        };
        let expect = request.expect.clone().unwrap_or_default();

        let idle_timeout = request
            .timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.idle_timeout_ms)
            .unwrap_or(self.session_config.idle_timeout_ms);
        let connect_timeout_ms = request
            .timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.connect_timeout_ms)
            .unwrap_or(15_000);

        let backend: Box<dyn SessionBackend> = match request.protocol {
            Protocol::Ssh => {
                let backend = SshBackend::connect(SshConnectParams {
                    session_id: &id,
                    host: &request.host,
                    port,
                    username: request.username.clone(),
                    auth: request.auth.clone(),
                    options: request.ssh_options.clone(),
                    ssh_config: &self.ssh_config,
                    connect_timeout_ms,
                    pty: pty.clone(),
                    output: output.clone(),
                })
                .await?;
                Box::new(backend)
            }
            Protocol::Telnet => {
                let backend = TelnetBackend::connect(
                    &request.host,
                    port,
                    pty.clone(),
                    connect_timeout_ms,
                    output.clone(),
                )
                .await?;
                Box::new(backend)
            }
        };

        let session = Arc::new(Session::new(SessionInit {
            id: id.clone(),
            protocol: request.protocol,
            host: request.host.clone(),
            port,
            session_type,
            device_id: device_id.clone(),
            backend,
            buffer,
            notify,
            last_activity,
            bytes_in,
            bytes_out,
            expect,
            pty: pty.clone(),
            idle_timeout_ms: idle_timeout,
            telnet_line_ending: self.telnet_line_ending.clone(),
            record_tx_events: self.session_config.record_tx_events,
        }));

        self.sessions.write().await.insert(id.clone(), session);
        if session_type == SessionType::Console {
            if let Some(device_id) = device_id {
                self.console_sessions
                    .write()
                    .await
                    .insert(device_id, id.clone());
            }
        }

        if !self.cleanup_running.load(Ordering::SeqCst) {
            self.start_cleanup_task();
        }

        Ok(SessionOpenResponse {
            session_id: id,
            protocol: request.protocol,
            pty_enabled: pty.enabled,
            server_banner: None,
            security_warning: if request.protocol == Protocol::Telnet {
                Some("Telnet is cleartext; credentials and data are not encrypted.".to_string())
            } else {
                None
            },
            lock_acquired: None,
            existing_session_id: None,
        })
    }

    pub async fn get_session(&self, session_id: &str) -> PtyResult<Arc<Session>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "Session not found").into())
    }

    pub async fn close_session(&self, session_id: &str, force: bool) -> PtyResult<()> {
        let session = self.get_session(session_id).await?;
        if session.state() == SessionState::Closed {
            return Ok(());
        }
        let session_type = session.session_type;
        let device_id = session.device_id.clone();
        session.close(force).await?;
        self.sessions.write().await.remove(session_id);
        if session_type == SessionType::Console {
            if let Some(device_id) = device_id {
                self.console_sessions.write().await.remove(&device_id);
            }
        }
        Ok(())
    }

    pub async fn list_sessions(&self) -> SessionListResponse {
        let sessions = self.sessions.read().await;
        let mut entries = Vec::with_capacity(sessions.len());
        for session in sessions.values() {
            let (created_at, last_activity, bytes_in, bytes_out) = session.metrics();
            entries.push(SessionListEntry {
                session_id: session.id.clone(),
                protocol: session.protocol,
                host: session.host.clone(),
                port: session.port,
                created_at,
                last_activity_at: last_activity,
                bytes_in,
                bytes_out,
                state: session.state(),
                session_type: session.session_type,
                device_id: session.device_id.clone(),
            });
        }
        SessionListResponse { sessions: entries }
    }

    pub async fn cleanup_idle_sessions(&self) {
        let now = now_ms();
        let mut to_close = Vec::new();
        {
            let sessions = self.sessions.read().await;
            for (id, session) in sessions.iter() {
                if session.idle_timeout_ms == 0 {
                    continue;
                }
                let last_activity = session.last_activity.load(Ordering::SeqCst);
                if now.saturating_sub(last_activity) > session.idle_timeout_ms {
                    to_close.push(id.clone());
                }
            }
        }
        for id in to_close {
            let _ = self.close_session(&id, true).await;
        }
    }

    fn start_cleanup_task(self: &Arc<Self>) {
        if self.cleanup_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                manager.cleanup_idle_sessions().await;
                sleep(Duration::from_secs(30)).await;
            }
        });
    }
}

fn normalize_telnet_line_endings(bytes: &[u8], mode: TelnetLineEnding) -> Vec<u8> {
    match mode {
        TelnetLineEnding::PassThrough => bytes.to_vec(),
        TelnetLineEnding::Lf => bytes.to_vec(),
        TelnetLineEnding::Cr => bytes
            .iter()
            .map(|&b| if b == b'\n' { b'\r' } else { b })
            .collect(),
        TelnetLineEnding::Crlf => {
            let mut out = Vec::with_capacity(bytes.len());
            for &b in bytes {
                if b == b'\n' {
                    out.push(b'\r');
                    out.push(b'\n');
                } else {
                    out.push(b);
                }
            }
            out
        }
    }
}

fn key_bytes(protocol: Protocol, key: SessionKey) -> PtyResult<Vec<u8>> {
    let bytes = match key {
        SessionKey::Enter => match protocol {
            Protocol::Telnet => vec![b'\r'],
            Protocol::Ssh => vec![b'\n'],
        },
        SessionKey::Tab => vec![b'\t'],
        SessionKey::Backspace => vec![0x7f],
        SessionKey::Delete => vec![0x1b, b'[', b'3', b'~'],
        SessionKey::Home => vec![0x1b, b'[', b'H'],
        SessionKey::End => vec![0x1b, b'[', b'F'],
        SessionKey::CtrlC => vec![0x03],
        SessionKey::CtrlD => vec![0x04],
        SessionKey::CtrlZ => vec![0x1a],
        SessionKey::CtrlBackslash => vec![0x1c],
        SessionKey::CtrlA => vec![0x01],
        SessionKey::CtrlE => vec![0x05],
        SessionKey::CtrlK => vec![0x0b],
        SessionKey::CtrlU => vec![0x15],
        SessionKey::CtrlL => vec![0x0c],
        SessionKey::Esc => vec![0x1b],
        SessionKey::ArrowUp => vec![0x1b, b'[', b'A'],
        SessionKey::ArrowDown => vec![0x1b, b'[', b'B'],
        SessionKey::ArrowLeft => vec![0x1b, b'[', b'D'],
        SessionKey::ArrowRight => vec![0x1b, b'[', b'C'],
        SessionKey::PageUp => vec![0x1b, b'[', b'5', b'~'],
        SessionKey::PageDown => vec![0x1b, b'[', b'6', b'~'],
    };
    Ok(bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

pub struct ReadResult {
    pub slice: BufferSlice,
    pub matched: bool,
    pub idle_reached: bool,
    pub timed_out: bool,
    pub eof: bool,
    pub waiting_for_input: Option<bool>,
    pub next_cursor: u64,
}

pub struct ReadParams {
    pub cursor: Option<u64>,
    pub timeout_ms: u64,
    pub max_bytes: usize,
    pub until_regex: Option<Regex>,
    pub include_match: bool,
    pub until_idle_ms: Option<u64>,
    pub input_hints: Option<Vec<Regex>>,
}

pub async fn read_from_session(
    session: &Arc<Session>,
    params: ReadParams,
) -> PtyResult<ReadResult> {
    let start_cursor = params.cursor.unwrap_or_else(|| session.buffer_end_cursor());
    let deadline = Instant::now() + Duration::from_millis(params.timeout_ms);
    let mut idle_deadline = params
        .until_idle_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));
    let mut current_cursor = start_cursor;

    loop {
        let slice = {
            let buffer = session.lock_buffer();
            buffer.slice_from(current_cursor, params.max_bytes)
        };
        if slice.truncated && slice.bytes.is_empty() {
            current_cursor = slice.start_cursor;
        }
        let eof = session.is_eof();

        if !slice.bytes.is_empty() {
            let mut matched = false;
            let mut bytes = slice.bytes.clone();
            if let Some(regex) = &params.until_regex {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if let Some(mat) = regex.find(text) {
                        matched = true;
                        let end = if params.include_match {
                            mat.end()
                        } else {
                            mat.start()
                        };
                        bytes.truncate(end);
                    }
                }
            }

            let waiting_for_input = params.input_hints.as_ref().map(|hints| {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    hints.iter().any(|regex| regex.is_match(text))
                } else {
                    false
                }
            });

            let effective_cursor = if slice.truncated {
                slice.start_cursor
            } else {
                current_cursor
            };
            let next_cursor = effective_cursor + bytes.len() as u64;
            return Ok(ReadResult {
                slice: BufferSlice { bytes, ..slice },
                matched,
                idle_reached: false,
                timed_out: false,
                eof,
                waiting_for_input,
                next_cursor,
            });
        }

        if eof {
            return Ok(ReadResult {
                slice,
                matched: false,
                idle_reached: false,
                timed_out: false,
                eof: true,
                waiting_for_input: None,
                next_cursor: current_cursor,
            });
        }

        if let Some(idle_deadline_at) = idle_deadline {
            if Instant::now() >= idle_deadline_at {
                return Ok(ReadResult {
                    slice,
                    matched: false,
                    idle_reached: true,
                    timed_out: false,
                    eof: false,
                    waiting_for_input: None,
                    next_cursor: current_cursor,
                });
            }
        }

        if Instant::now() >= deadline {
            return Ok(ReadResult {
                slice,
                matched: false,
                idle_reached: false,
                timed_out: true,
                eof: false,
                waiting_for_input: None,
                next_cursor: current_cursor,
            });
        }

        let notify = session.notify.notified();
        let next_wait = {
            let mut next = deadline;
            if let Some(idle) = idle_deadline {
                if idle < next {
                    next = idle;
                }
            }
            next
        };

        tokio::select! {
            _ = notify => {
                current_cursor = current_cursor.max(session.buffer_start_cursor());
                idle_deadline = params
                    .until_idle_ms
                    .map(|ms| Instant::now() + Duration::from_millis(ms));
            }
            _ = tokio::time::sleep_until(next_wait) => {}
        }
    }
}

pub fn encode_chunk(bytes: &[u8], requested: Encoding) -> (String, Encoding) {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    match requested {
        Encoding::Base64 => (STANDARD.encode(bytes), Encoding::Base64),
        Encoding::Utf8 => match std::str::from_utf8(bytes) {
            Ok(text) => (text.to_string(), Encoding::Utf8),
            Err(_) => (STANDARD.encode(bytes), Encoding::Base64),
        },
    }
}

pub fn parse_cursor(cursor: &str) -> PtyResult<u64> {
    cursor
        .parse::<u64>()
        .map_err(|_| ApiError::new(ErrorCode::InvalidArgument, "Invalid cursor value").into())
}

pub fn format_cursor(cursor: u64) -> String {
    cursor.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    #[test]
    fn normalize_telnet_line_endings_cr() {
        let input = b"show\nrun\n";
        let out = normalize_telnet_line_endings(input, TelnetLineEnding::Cr);
        assert_eq!(out, b"show\rrun\r");
    }

    #[test]
    fn key_bytes_enter_telnet() {
        let bytes = key_bytes(Protocol::Telnet, SessionKey::Enter).expect("key bytes");
        assert_eq!(bytes, vec![b'\r']);
    }

    #[test]
    fn session_key_parses_canonical_and_aliases() {
        let cases = [
            ("enter", SessionKey::Enter),
            ("tab", SessionKey::Tab),
            ("backspace", SessionKey::Backspace),
            ("delete", SessionKey::Delete),
            ("home", SessionKey::Home),
            ("end", SessionKey::End),
            ("ctrl_c", SessionKey::CtrlC),
            ("ctrl+c", SessionKey::CtrlC),
            ("ctrl-c", SessionKey::CtrlC),
            ("ctrl_d", SessionKey::CtrlD),
            ("ctrl+d", SessionKey::CtrlD),
            ("ctrl-d", SessionKey::CtrlD),
            ("ctrl_z", SessionKey::CtrlZ),
            ("ctrl+z", SessionKey::CtrlZ),
            ("ctrl-z", SessionKey::CtrlZ),
            ("ctrl_backslash", SessionKey::CtrlBackslash),
            ("ctrl+backslash", SessionKey::CtrlBackslash),
            ("ctrl-backslash", SessionKey::CtrlBackslash),
            ("ctrl+\\\\", SessionKey::CtrlBackslash),
            ("ctrl-\\\\", SessionKey::CtrlBackslash),
            ("ctrl_a", SessionKey::CtrlA),
            ("ctrl+a", SessionKey::CtrlA),
            ("ctrl-a", SessionKey::CtrlA),
            ("ctrl_e", SessionKey::CtrlE),
            ("ctrl+e", SessionKey::CtrlE),
            ("ctrl-e", SessionKey::CtrlE),
            ("ctrl_k", SessionKey::CtrlK),
            ("ctrl+k", SessionKey::CtrlK),
            ("ctrl-k", SessionKey::CtrlK),
            ("ctrl_u", SessionKey::CtrlU),
            ("ctrl+u", SessionKey::CtrlU),
            ("ctrl-u", SessionKey::CtrlU),
            ("ctrl_l", SessionKey::CtrlL),
            ("ctrl+l", SessionKey::CtrlL),
            ("ctrl-l", SessionKey::CtrlL),
            ("esc", SessionKey::Esc),
            ("arrow_up", SessionKey::ArrowUp),
            ("arrow-up", SessionKey::ArrowUp),
            ("arrow_down", SessionKey::ArrowDown),
            ("arrow-down", SessionKey::ArrowDown),
            ("arrow_left", SessionKey::ArrowLeft),
            ("arrow-left", SessionKey::ArrowLeft),
            ("arrow_right", SessionKey::ArrowRight),
            ("arrow-right", SessionKey::ArrowRight),
            ("page_up", SessionKey::PageUp),
            ("page-up", SessionKey::PageUp),
            ("page_down", SessionKey::PageDown),
            ("page-down", SessionKey::PageDown),
        ];

        for (raw, expected) in cases {
            let value = format!("\"{}\"", raw);
            let parsed: SessionKey =
                serde_json::from_str(&value).unwrap_or_else(|_| panic!("parse {}", raw));
            assert_eq!(
                std::mem::discriminant(&parsed),
                std::mem::discriminant(&expected),
                "parsed {}",
                raw
            );
        }
    }

    #[test]
    fn key_bytes_match_expected_sequences() {
        let cases = [
            (SessionKey::Enter, vec![b'\n']),
            (SessionKey::Tab, vec![b'\t']),
            (SessionKey::Backspace, vec![0x7f]),
            (SessionKey::Delete, vec![0x1b, b'[', b'3', b'~']),
            (SessionKey::Home, vec![0x1b, b'[', b'H']),
            (SessionKey::End, vec![0x1b, b'[', b'F']),
            (SessionKey::CtrlC, vec![0x03]),
            (SessionKey::CtrlD, vec![0x04]),
            (SessionKey::CtrlZ, vec![0x1a]),
            (SessionKey::CtrlBackslash, vec![0x1c]),
            (SessionKey::CtrlA, vec![0x01]),
            (SessionKey::CtrlE, vec![0x05]),
            (SessionKey::CtrlK, vec![0x0b]),
            (SessionKey::CtrlU, vec![0x15]),
            (SessionKey::CtrlL, vec![0x0c]),
            (SessionKey::Esc, vec![0x1b]),
            (SessionKey::ArrowUp, vec![0x1b, b'[', b'A']),
            (SessionKey::ArrowDown, vec![0x1b, b'[', b'B']),
            (SessionKey::ArrowLeft, vec![0x1b, b'[', b'D']),
            (SessionKey::ArrowRight, vec![0x1b, b'[', b'C']),
            (SessionKey::PageUp, vec![0x1b, b'[', b'5', b'~']),
            (SessionKey::PageDown, vec![0x1b, b'[', b'6', b'~']),
        ];

        for (key, expected) in cases {
            let actual = key_bytes(Protocol::Ssh, key).expect("key bytes");
            assert_eq!(actual, expected);
        }
    }

    struct DummyBackend {
        eof: Arc<AtomicBool>,
    }

    #[async_trait]
    impl SessionBackend for DummyBackend {
        async fn write(&self, _data: &[u8]) -> PtyResult<usize> {
            Ok(0)
        }

        async fn resize(&self, _cols: u16, _rows: u16) -> PtyResult<()> {
            Ok(())
        }

        async fn close(&self, _force: bool) -> PtyResult<()> {
            Ok(())
        }

        fn is_eof(&self) -> bool {
            self.eof.load(Ordering::SeqCst)
        }
    }

    fn build_session(session_type: SessionType) -> Arc<Session> {
        let buffer = Arc::new(Mutex::new(OutputBuffer::new(1024, 100)));
        let notify = Arc::new(Notify::new());
        let last_activity = Arc::new(AtomicU64::new(now_ms()));
        let bytes_in = Arc::new(AtomicU64::new(0));
        let bytes_out = Arc::new(AtomicU64::new(0));
        let backend = DummyBackend {
            eof: Arc::new(AtomicBool::new(false)),
        };
        let pty = PtyOptions::default();
        let device_id = if session_type == SessionType::Console {
            Some("device-1".to_string())
        } else {
            None
        };
        Arc::new(Session::new(SessionInit {
            id: "test".to_string(),
            protocol: Protocol::Ssh,
            host: "localhost".to_string(),
            port: 22,
            session_type,
            device_id,
            backend: Box::new(backend),
            buffer,
            notify,
            last_activity,
            bytes_in,
            bytes_out,
            expect: ExpectConfig::default(),
            pty,
            idle_timeout_ms: 0,
            telnet_line_ending: TelnetLineEnding::Cr,
            record_tx_events: false,
        }))
    }

    #[tokio::test]
    async fn read_from_session_returns_output() {
        let session = build_session(SessionType::Normal);
        session.append_output(b"hello");
        let read = read_from_session(
            &session,
            ReadParams {
                cursor: Some(0),
                timeout_ms: 1000,
                max_bytes: 1024,
                until_regex: None,
                include_match: true,
                until_idle_ms: None,
                input_hints: None,
            },
        )
        .await
        .expect("read");
        assert_eq!(read.slice.bytes, b"hello");
    }

    #[tokio::test]
    async fn lock_blocks_other_tasks() {
        let session = build_session(SessionType::Normal);
        session.lock("task-1", 1000).await.expect("lock");
        assert!(session.ensure_write_access(Some("task-1")).await.is_ok());
        assert!(session.ensure_write_access(Some("task-2")).await.is_err());
        assert!(session.ensure_write_access(None).await.is_err());
    }

    #[tokio::test]
    async fn console_requires_lock_for_write() {
        let session = build_session(SessionType::Console);
        assert!(session.ensure_write_access(Some("task-1")).await.is_err());
        session.lock("task-1", 1000).await.expect("lock");
        assert!(session.ensure_write_access(Some("task-1")).await.is_ok());
    }

    #[test]
    fn encoding_accepts_utf8_aliases() {
        let encoded = serde_json::from_str::<Encoding>("\"utf-8\"").expect("utf-8");
        assert!(matches!(encoded, Encoding::Utf8));
        let encoded = serde_json::from_str::<Encoding>("\"utf8\"").expect("utf8");
        assert!(matches!(encoded, Encoding::Utf8));
    }
}
