use crate::config::{ControlMode, SessionConfig};
use crate::error::{ApiError, ErrorCode, PtyError, PtyResult};
use crate::session::{
    Capabilities, ConfigAction, Encoding, InputHints, IoAction, RcMode, ReadMode, ReadParams,
    SessionAction, SessionConfigRequest, SessionConfigResponse, SessionExecRequest,
    SessionExecResponse, SessionIoRequest, SessionIoResponse, SessionManager, SessionOpenRequest,
    SessionOpenResponse, SessionReadRequest, SessionReadResponse, SessionRequest, SessionResponse,
    SessionTailRequest, SessionTailResponse, encode_chunk, format_cursor, parse_cursor,
    read_from_session,
};
use axum::{
    Router,
    http::{
        HeaderValue, StatusCode,
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
    },
    middleware,
    response::IntoResponse,
};
use regex::Regex;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use rmcp::{ErrorData as McpError, ServiceExt, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Duration, Instant};
use uuid::Uuid;

const DEFAULT_LOCK_TTL_MS: u64 = 60_000;

#[derive(Clone)]
pub struct McpServer {
    session_manager: Arc<SessionManager>,
    session_config: SessionConfig,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(session_manager: Arc<SessionManager>, session_config: SessionConfig) -> Self {
        Self {
            session_manager,
            session_config,
            tool_router: Self::tool_router(),
        }
    }

    pub async fn serve_stdio(self) -> PtyResult<()> {
        let running = self.serve(stdio()).await.map_err(|err| {
            ApiError::new(ErrorCode::IoError, "MCP stdio initialization failed")
                .with_details(err.to_string())
        })?;
        running.waiting().await.map_err(|err| {
            ApiError::new(ErrorCode::IoError, "MCP stdio task failed").with_details(err.to_string())
        })?;
        Ok(())
    }

    pub async fn serve_http(self, listen: &str, auth_token: &str) -> PtyResult<()> {
        let addr: SocketAddr = listen.parse().map_err(|_| {
            ApiError::new(ErrorCode::InvalidArgument, "Invalid HTTP listen address")
        })?;
        let session_manager = Arc::new(LocalSessionManager::default());
        let config = StreamableHttpServerConfig::default();
        let service_factory = {
            let server = self.clone();
            move || Ok(server.clone())
        };
        let service = StreamableHttpService::new(service_factory, session_manager, config);
        let auth_token = auth_token.to_string();
        let router = Router::new()
            .route_service("/mcp", service)
            .layer(middleware::from_fn(
                move |req: axum::http::Request<axum::body::Body>, next: axum::middleware::Next| {
                    let auth_token = auth_token.clone();
                    async move {
                        if auth_token.is_empty() {
                            return next.run(req).await;
                        }
                        let expected = format!("Bearer {}", auth_token);
                        let authorized = req
                            .headers()
                            .get(AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value == expected);
                        if authorized {
                            next.run(req).await
                        } else {
                            let mut response =
                                (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
                            response
                                .headers_mut()
                                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
                            response
                        }
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|err| {
            ApiError::new(ErrorCode::IoError, "HTTP bind failed").with_details(err.to_string())
        })?;
        axum::serve(listener, router).await.map_err(|err| {
            ApiError::new(ErrorCode::IoError, "HTTP server failed").with_details(err.to_string())
        })?;
        Ok(())
    }

    async fn open_with_lock(&self, req: SessionOpenRequest) -> PtyResult<SessionOpenResponse> {
        let acquire_lock = req.acquire_lock.unwrap_or(false);
        if acquire_lock && req.task_id.is_none() {
            return Err(ApiError::new(
                ErrorCode::InvalidArgument,
                "task_id is required to acquire a lock",
            )
            .into());
        }
        let mut response = self.session_manager.open_session(req.clone()).await?;
        if acquire_lock {
            if response.existing_session_id.is_none() {
                let task_id = req.task_id.as_deref().expect("task_id checked before");
                let ttl_ms = req.lock_ttl_ms.unwrap_or(DEFAULT_LOCK_TTL_MS);
                let session = self
                    .session_manager
                    .get_session(&response.session_id)
                    .await?;
                session.lock(task_id, ttl_ms).await?;
                response.lock_acquired = Some(true);
            } else {
                response.lock_acquired = Some(false);
            }
        }
        Ok(response)
    }

    async fn handle_session(&self, req: SessionRequest) -> PtyResult<SessionResponse> {
        match req.action {
            SessionAction::Open => {
                let protocol = req.protocol.ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "protocol is required")
                })?;
                let host = req
                    .host
                    .clone()
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidArgument, "host is required"))?;
                let open_req = SessionOpenRequest {
                    protocol,
                    host,
                    port: req.port,
                    username: req.username,
                    auth: req.auth,
                    pty: req.pty,
                    timeouts: req.timeouts,
                    ssh_options: req.ssh_options,
                    expect: req.expect,
                    session_type: req.session_type,
                    device_id: req.device_id,
                    acquire_lock: req.acquire_lock,
                    lock_ttl_ms: req.lock_ttl_ms,
                    task_id: req.task_id.clone(),
                };
                let response = self.open_with_lock(open_req).await?;
                Ok(SessionResponse {
                    action: SessionAction::Open,
                    success: true,
                    session_id: Some(response.session_id.clone()),
                    protocol: Some(response.protocol),
                    pty_enabled: Some(response.pty_enabled),
                    security_warning: response.security_warning.clone(),
                    lock_acquired: response.lock_acquired,
                    existing_session_id: response.existing_session_id.clone(),
                    sessions: None,
                    capabilities: None,
                    lock_holder: None,
                    lock_expires_at: None,
                    message: None,
                })
            }
            SessionAction::Close => {
                let session_id = req.session_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "session_id is required")
                })?;
                let force = req.force.unwrap_or(false);
                self.session_manager
                    .close_session(session_id, force)
                    .await?;
                Ok(SessionResponse {
                    action: SessionAction::Close,
                    success: true,
                    session_id: Some(session_id.to_string()),
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: None,
                    existing_session_id: None,
                    sessions: None,
                    capabilities: None,
                    lock_holder: None,
                    lock_expires_at: None,
                    message: None,
                })
            }
            SessionAction::List => {
                let list = self.session_manager.list_sessions().await;
                Ok(SessionResponse {
                    action: SessionAction::List,
                    success: true,
                    session_id: None,
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: None,
                    existing_session_id: None,
                    sessions: Some(list.sessions),
                    capabilities: Some(session_capabilities()),
                    lock_holder: None,
                    lock_expires_at: None,
                    message: None,
                })
            }
            SessionAction::Lock => {
                let session_id = req.session_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "session_id is required")
                })?;
                let task_id = req.task_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "task_id is required")
                })?;
                let ttl_ms = req.lock_ttl_ms.unwrap_or(DEFAULT_LOCK_TTL_MS);
                let session = self.session_manager.get_session(session_id).await?;
                let info = session.lock(task_id, ttl_ms).await?;
                Ok(SessionResponse {
                    action: SessionAction::Lock,
                    success: true,
                    session_id: Some(session_id.to_string()),
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: Some(true),
                    existing_session_id: None,
                    sessions: None,
                    capabilities: None,
                    lock_holder: Some(info.task_id),
                    lock_expires_at: Some(info.expires_at),
                    message: None,
                })
            }
            SessionAction::Unlock => {
                let session_id = req.session_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "session_id is required")
                })?;
                let task_id = req.task_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "task_id is required")
                })?;
                let session = self.session_manager.get_session(session_id).await?;
                session.unlock(task_id).await?;
                Ok(SessionResponse {
                    action: SessionAction::Unlock,
                    success: true,
                    session_id: Some(session_id.to_string()),
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: None,
                    existing_session_id: None,
                    sessions: None,
                    capabilities: None,
                    lock_holder: None,
                    lock_expires_at: None,
                    message: None,
                })
            }
            SessionAction::Heartbeat => {
                let session_id = req.session_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "session_id is required")
                })?;
                let task_id = req.task_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "task_id is required")
                })?;
                let session = self.session_manager.get_session(session_id).await?;
                let info = session.heartbeat(task_id, req.lock_ttl_ms).await?;
                Ok(SessionResponse {
                    action: SessionAction::Heartbeat,
                    success: true,
                    session_id: Some(session_id.to_string()),
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: None,
                    existing_session_id: None,
                    sessions: None,
                    capabilities: None,
                    lock_holder: Some(info.task_id),
                    lock_expires_at: Some(info.expires_at),
                    message: None,
                })
            }
            SessionAction::Status => {
                let session_id = req.session_id.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "session_id is required")
                })?;
                let session = self.session_manager.get_session(session_id).await?;
                let info = session.lock_status().await;
                Ok(SessionResponse {
                    action: SessionAction::Status,
                    success: true,
                    session_id: Some(session_id.to_string()),
                    protocol: None,
                    pty_enabled: None,
                    security_warning: None,
                    lock_acquired: None,
                    existing_session_id: None,
                    sessions: None,
                    capabilities: None,
                    lock_holder: info.as_ref().map(|lock| lock.task_id.clone()),
                    lock_expires_at: info.map(|lock| lock.expires_at),
                    message: None,
                })
            }
        }
    }

    async fn handle_session_io(&self, req: SessionIoRequest) -> PtyResult<SessionIoResponse> {
        let session = self.session_manager.get_session(&req.session_id).await?;
        match req.action {
            IoAction::Write => {
                session.ensure_write_access(req.task_id.as_deref()).await?;
                match (&req.data, &req.key) {
                    (Some(_), Some(_)) => {
                        return Err(ApiError::new(
                            ErrorCode::InvalidArgument,
                            "Specify either data or key, not both",
                        )
                        .into());
                    }
                    (None, None) => {
                        return Err(ApiError::new(
                            ErrorCode::InvalidArgument,
                            "data or key is required",
                        )
                        .into());
                    }
                    _ => {}
                }
                let sensitive = req.sensitive.unwrap_or(false);
                let bytes_written = if let Some(data) = &req.data {
                    let encoding = req.encoding.unwrap_or_default();
                    let bytes = decode_payload(data, encoding)?;
                    session.write(&bytes, sensitive).await?
                } else if let Some(key) = req.key {
                    session.send_key(key).await?
                } else {
                    0
                };
                Ok(SessionIoResponse {
                    action: IoAction::Write,
                    bytes_written: Some(bytes_written),
                    chunk: None,
                    encoding: None,
                    next_cursor: None,
                    buffer_start_cursor: None,
                    buffer_end_cursor: None,
                    matched: None,
                    idle_reached: None,
                    timed_out: None,
                    eof: None,
                    waiting_for_input: None,
                    truncated: None,
                    dropped_bytes: None,
                    buffered_bytes: None,
                    buffer_limit_bytes: None,
                })
            }
            IoAction::Read => {
                let mode = req.mode.unwrap_or(ReadMode::Cursor);
                match mode {
                    ReadMode::Cursor => {
                        let read_req = SessionReadRequest {
                            session_id: req.session_id,
                            cursor: req.cursor,
                            timeout_ms: req.timeout_ms,
                            max_bytes: req.max_bytes,
                            until_regex: req.until_regex,
                            include_match: req.include_match,
                            until_idle_ms: req.until_idle_ms,
                            encoding: req.encoding,
                            input_hints: req.input_hints,
                        };
                        let read = self.handle_read(read_req).await?;
                        Ok(SessionIoResponse {
                            action: IoAction::Read,
                            bytes_written: None,
                            chunk: Some(read.chunk),
                            encoding: Some(read.encoding),
                            next_cursor: Some(read.next_cursor),
                            buffer_start_cursor: Some(read.buffer_start_cursor),
                            buffer_end_cursor: Some(read.buffer_end_cursor),
                            matched: Some(read.matched),
                            idle_reached: Some(read.idle_reached),
                            timed_out: Some(read.timed_out),
                            eof: Some(read.eof),
                            waiting_for_input: read.waiting_for_input,
                            truncated: Some(read.truncated),
                            dropped_bytes: Some(read.dropped_bytes),
                            buffered_bytes: Some(read.buffered_bytes),
                            buffer_limit_bytes: Some(read.buffer_limit_bytes),
                        })
                    }
                    ReadMode::Tail => {
                        let tail_req = SessionTailRequest {
                            session_id: req.session_id,
                            max_bytes: req.max_bytes,
                            max_lines: req.max_lines,
                            encoding: req.encoding,
                        };
                        let tail = self.handle_tail(tail_req).await?;
                        Ok(SessionIoResponse {
                            action: IoAction::Read,
                            bytes_written: None,
                            chunk: Some(tail.tail),
                            encoding: Some(tail.encoding),
                            next_cursor: Some(tail.next_cursor),
                            buffer_start_cursor: Some(tail.buffer_start_cursor),
                            buffer_end_cursor: Some(tail.buffer_end_cursor),
                            matched: None,
                            idle_reached: None,
                            timed_out: None,
                            eof: None,
                            waiting_for_input: None,
                            truncated: Some(tail.truncated),
                            dropped_bytes: None,
                            buffered_bytes: Some(tail.buffered_bytes),
                            buffer_limit_bytes: Some(tail.buffer_limit_bytes),
                        })
                    }
                }
            }
        }
    }

    async fn handle_session_config(
        &self,
        req: SessionConfigRequest,
    ) -> PtyResult<SessionConfigResponse> {
        let session = self.session_manager.get_session(&req.session_id).await?;
        match req.action {
            ConfigAction::Resize => {
                let cols = req
                    .cols
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidArgument, "cols is required"))?;
                let rows = req
                    .rows
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidArgument, "rows is required"))?;
                session.resize(cols, rows).await?;
                Ok(SessionConfigResponse {
                    success: true,
                    cols: None,
                    rows: None,
                    expect: None,
                })
            }
            ConfigAction::Expect => {
                let expect = req.expect.ok_or_else(|| {
                    ApiError::new(ErrorCode::InvalidArgument, "expect is required")
                })?;
                session.set_expect(expect).await;
                Ok(SessionConfigResponse {
                    success: true,
                    cols: None,
                    rows: None,
                    expect: None,
                })
            }
            ConfigAction::Get => {
                let expect = session.expect().await;
                let (cols, rows) = session.pty_size();
                let (cols, rows) = if session.pty_enabled() {
                    (Some(cols), Some(rows))
                } else {
                    (None, None)
                };
                Ok(SessionConfigResponse {
                    success: true,
                    cols,
                    rows,
                    expect: Some(expect),
                })
            }
        }
    }

    async fn handle_read(&self, req: SessionReadRequest) -> PtyResult<SessionReadResponse> {
        let session = self.session_manager.get_session(&req.session_id).await?;
        let timeout_ms = req
            .timeout_ms
            .unwrap_or(self.session_config.default_read_timeout_ms);
        let max_bytes = req.max_bytes.unwrap_or(65536);
        let include_match = req.include_match.unwrap_or(true);

        let cursor = req.cursor.as_deref().map(parse_cursor).transpose()?;

        let until_regex = match req.until_regex {
            Some(pattern) => Some(Regex::new(&pattern)?),
            None => None,
        };

        let input_hints = compile_input_hints(req.input_hints)?;
        let read = read_from_session(
            &session,
            ReadParams {
                cursor,
                timeout_ms,
                max_bytes,
                until_regex,
                include_match,
                until_idle_ms: req.until_idle_ms,
                input_hints,
            },
        )
        .await?;

        let (text, actual_encoding) =
            encode_chunk(&read.slice.bytes, req.encoding.unwrap_or_default());
        Ok(SessionReadResponse {
            chunk: text,
            encoding: actual_encoding,
            next_cursor: format_cursor(read.next_cursor),
            buffer_start_cursor: format_cursor(read.slice.start_cursor),
            buffer_end_cursor: format_cursor(read.slice.end_cursor),
            matched: read.matched,
            idle_reached: read.idle_reached,
            timed_out: read.timed_out,
            eof: read.eof,
            waiting_for_input: read.waiting_for_input,
            truncated: read.slice.truncated,
            dropped_bytes: read.slice.dropped_bytes,
            buffered_bytes: read.slice.buffered_bytes,
            buffer_limit_bytes: read.slice.buffer_limit_bytes,
        })
    }

    async fn handle_tail(&self, req: SessionTailRequest) -> PtyResult<SessionTailResponse> {
        let session = self.session_manager.get_session(&req.session_id).await?;
        let max_bytes = req.max_bytes.unwrap_or(65536);
        let max_lines = req.max_lines;
        let encoding = req.encoding.unwrap_or_default();
        let tail = session.tail(max_bytes, max_lines);
        let (tail_text, actual_encoding) = encode_chunk(&tail.bytes, encoding);
        Ok(SessionTailResponse {
            tail: tail_text,
            encoding: actual_encoding,
            tail_start_cursor: format_cursor(tail.start_cursor),
            next_cursor: format_cursor(tail.end_cursor),
            buffer_start_cursor: format_cursor(session.buffer_start_cursor()),
            buffer_end_cursor: format_cursor(session.buffer_end_cursor()),
            truncated: tail.truncated,
            buffered_bytes: tail.buffered_bytes,
            buffer_limit_bytes: tail.buffer_limit_bytes,
        })
    }

    async fn handle_exec(&self, req: SessionExecRequest) -> PtyResult<SessionExecResponse> {
        let session = self.session_manager.get_session(&req.session_id).await?;
        session.ensure_write_access(req.task_id.as_deref()).await?;
        let timeout_ms = req
            .timeout_ms
            .unwrap_or(self.session_config.default_exec_timeout_ms);
        let until_idle_ms = req.until_idle_ms;
        let rc_mode = req.rc_mode.unwrap_or(RcMode {
            enabled: Some(true),
            marker_prefix: None,
            marker_suffix: None,
        });
        let uses_default_markers =
            rc_mode.marker_prefix.is_none() && rc_mode.marker_suffix.is_none();
        let marker_prefix = rc_mode
            .marker_prefix
            .unwrap_or_else(|| "\u{001e}RC=".to_string());
        let marker_suffix = rc_mode
            .marker_suffix
            .unwrap_or_else(|| "\u{001f}".to_string());
        let rc_enabled = rc_mode.enabled.unwrap_or(true);
        let fallback_marker = if rc_enabled && uses_default_markers {
            let token = Uuid::new_v4().simple().to_string();
            Some((format!("PTYCTL_RC_{}=", token), format!(":END_{}", token)))
        } else {
            None
        };

        let expect = match req.expect {
            Some(expect) => expect,
            None => session.expect().await,
        };

        let prompt_regex = expect.prompt_regex.as_deref().map(Regex::new).transpose()?;
        let mut prompt_detected = prompt_regex.as_ref().map(|_| false);

        let error_regexes = expect
            .error_regexes
            .unwrap_or_default()
            .into_iter()
            .map(|pattern| Regex::new(&pattern))
            .collect::<Result<Vec<_>, _>>()?;

        let start_cursor = session.buffer_end_cursor();
        let mut command = req.cmd;
        if rc_enabled {
            command = format!(
                "{}; rc=$?; printf \\\"\\n{}%d{}\\n\\\" \\\"$rc\\\"",
                command, marker_prefix, marker_suffix
            );
            if let Some((ref fb_prefix, ref fb_suffix)) = fallback_marker {
                command.push_str(&format!(
                    "; printf \\\"\\n{}%d{}\\n\\\" \\\"$rc\\\"",
                    fb_prefix, fb_suffix
                ));
            }
        }
        command.push('\n');
        session.write(command.as_bytes(), false).await?;

        let mut collected = Vec::new();
        let mut cursor = start_cursor;
        let start_time = Instant::now();
        let deadline = start_time + Duration::from_millis(timeout_ms);
        let mut timed_out = false;

        let done_reason = loop {
            let now = Instant::now();
            if now >= deadline {
                timed_out = true;
                break "timeout";
            }
            let remaining_ms = (deadline - now).as_millis() as u64;
            let until_regex = if rc_enabled {
                Some(build_marker_regex(
                    &marker_prefix,
                    &marker_suffix,
                    fallback_marker.as_ref(),
                )?)
            } else {
                prompt_regex.clone()
            };

            let read = read_from_session(
                &session,
                ReadParams {
                    cursor: Some(cursor),
                    timeout_ms: remaining_ms,
                    max_bytes: 65536,
                    until_regex,
                    include_match: true,
                    until_idle_ms,
                    input_hints: None,
                },
            )
            .await?;
            collected.extend_from_slice(&read.slice.bytes);
            cursor = read.next_cursor;

            if read.matched {
                if rc_enabled {
                    break "marker_seen";
                } else {
                    prompt_detected = Some(true);
                    break "prompt_seen";
                }
            }
            if read.idle_reached {
                break "idle_reached";
            }
            if read.timed_out {
                timed_out = true;
                break "timeout";
            }
            if read.eof {
                break "eof";
            }
        };
        let done_reason = done_reason.to_string();
        let output_text = String::from_utf8_lossy(&collected).to_string();
        let (stdout, exit_code, exit_code_reason) = if rc_enabled {
            extract_exit_code(
                &output_text,
                &marker_prefix,
                &marker_suffix,
                fallback_marker.as_ref(),
            )
        } else {
            (output_text, None, Some("unsupported".to_string()))
        };

        let error_hints = extract_error_hints(&stdout, &error_regexes);
        Ok(SessionExecResponse {
            stdout,
            stderr: String::new(),
            exit_code,
            exit_code_reason,
            done_reason,
            prompt_detected,
            error_hints: if error_hints.is_empty() {
                None
            } else {
                Some(error_hints)
            },
            timed_out,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    async fn handle_control_request(&self, request: ControlRpcRequest) -> ControlRpcResponse {
        let id = request.id.clone().unwrap_or(Value::Null);
        let result = self.dispatch_control_method(request).await;
        match result {
            Ok(value) => ControlRpcResponse::success(id, value),
            Err(err) => ControlRpcResponse::error(id, err),
        }
    }

    pub(crate) async fn handle_control_request_filtered(
        &self,
        request: ControlRpcRequest,
        mode: ControlMode,
    ) -> ControlRpcResponse {
        if matches!(mode, ControlMode::Readonly) && !self.is_readonly_control_request(&request) {
            return ControlRpcResponse::error(
                request.id.clone().unwrap_or(Value::Null),
                ApiError::new(ErrorCode::Unsupported, "Control mode is readonly").into(),
            );
        }
        self.handle_control_request(request).await
    }

    fn is_readonly_control_request(&self, request: &ControlRpcRequest) -> bool {
        match request.method.as_str() {
            "ptyctl_session" => request
                .params
                .as_ref()
                .and_then(|params| serde_json::from_value::<SessionRequest>(params.clone()).ok())
                .is_some_and(|req| {
                    matches!(req.action, SessionAction::List | SessionAction::Status)
                }),
            "ptyctl_session_io" => request
                .params
                .as_ref()
                .and_then(|params| serde_json::from_value::<SessionIoRequest>(params.clone()).ok())
                .is_some_and(|req| matches!(req.action, IoAction::Read)),
            "ptyctl_session_config" => request
                .params
                .as_ref()
                .and_then(|params| {
                    serde_json::from_value::<SessionConfigRequest>(params.clone()).ok()
                })
                .is_some_and(|req| matches!(req.action, ConfigAction::Get)),
            _ => false,
        }
    }

    async fn dispatch_control_method(&self, request: ControlRpcRequest) -> PtyResult<Value> {
        let method = request.method.as_str();
        let params = request.params.unwrap_or(Value::Null);
        match method {
            "ptyctl_session" => {
                let req: SessionRequest = serde_json::from_value(params)?;
                let resp = self.handle_session(req).await?;
                Ok(serde_json::to_value(resp)?)
            }
            "ptyctl_session_io" => {
                let req: SessionIoRequest = serde_json::from_value(params)?;
                let resp = self.handle_session_io(req).await?;
                Ok(serde_json::to_value(resp)?)
            }
            "ptyctl_session_config" => {
                let req: SessionConfigRequest = serde_json::from_value(params)?;
                let resp = self.handle_session_config(req).await?;
                Ok(serde_json::to_value(resp)?)
            }
            "ptyctl_session_exec" => {
                let req: SessionExecRequest = serde_json::from_value(params)?;
                let resp = self.handle_exec(req).await?;
                Ok(serde_json::to_value(resp)?)
            }
            _ => Err(ApiError::new(ErrorCode::InvalidArgument, "Unknown method").into()),
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        name = "ptyctl_session",
        description = "Session lifecycle management (open/close/list/lock/unlock/heartbeat/status). For open: protocol is ssh or telnet; auth/pty/expect are objects (not JSON strings)."
    )]
    async fn session_tool(
        &self,
        params: Parameters<SessionRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self.handle_session(params.0).await.map_err(map_pty_error)?;
        structured_result(response)
    }

    #[tool(
        name = "ptyctl_session_io",
        description = "Unified session read/write interface. Use action=write with data or key; action=read supports cursor/tail and until_regex."
    )]
    async fn session_io_tool(
        &self,
        params: Parameters<SessionIoRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self
            .handle_session_io(params.0)
            .await
            .map_err(map_pty_error)?;
        structured_result(response)
    }

    #[tool(
        name = "ptyctl_session_config",
        description = "Manage session configuration (resize/expect/get)."
    )]
    async fn session_config_tool(
        &self,
        params: Parameters<SessionConfigRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self
            .handle_session_config(params.0)
            .await
            .map_err(map_pty_error)?;
        structured_result(response)
    }

    #[tool(
        name = "ptyctl_session_exec",
        description = "Execute a command in an existing session."
    )]
    async fn session_exec_tool(
        &self,
        params: Parameters<SessionExecRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self.handle_exec(params.0).await.map_err(map_pty_error)?;
        structured_result(response)
    }
}

#[tool_handler]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let server_info = Implementation {
            name: "ptyctl".to_string(),
            title: Some("ptyctl".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            icons: None,
            website_url: None,
        };
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some(
                concat!(
                    "Tool inputs are validated against the JSON schema; incorrect types or enum values return invalid_params.\n",
                    "Use ptyctl_session action=open to create a session_id; other tools require it.\n",
                    "Open parameters:\n",
                    "- protocol: \"ssh\" or \"telnet\" (no \"local\").\n",
                    "- auth: object (SshAuth). For password auth: {\"password\":\"...\"}. Do not pass JSON-encoded strings.\n",
                    "- pty: object with enabled/cols/rows/term; omit to use defaults.\n",
                    "- expect: object with optional prompt_regex/pager_regexes/error_regexes; do not pass a raw string.\n",
                    "- For action=open, protocol and host are required; for other actions, session_id is required.\n",
                    "- action=open only establishes the transport; use ptyctl_session_io to respond to login prompts.\n",
                    "Example (telnet): {\"action\":\"open\",\"protocol\":\"telnet\",\"host\":\"10.0.0.1\",\"port\":23,\"username\":\"admin\",\"auth\":{\"password\":\"...\"}}\n",
                    "Example (ssh password): {\"action\":\"open\",\"protocol\":\"ssh\",\"host\":\"10.0.0.1\",\"username\":\"root\",\"auth\":{\"password\":\"...\"}}\n",
                    "Example (expect): {\"action\":\"open\",\"protocol\":\"ssh\",\"host\":\"10.0.0.1\",\"expect\":{\"prompt_regex\":\"[#>$]\"}}\n",
                )
                .to_string(),
            ),
            server_info,
            ..Default::default()
        }
    }
}

fn structured_result<T: Serialize>(value: T) -> Result<CallToolResult, McpError> {
    let json_value = serde_json::to_value(value).map_err(|err| {
        McpError::internal_error(
            "Failed to serialize tool result",
            Some(json!({"details": err.to_string()})),
        )
    })?;
    Ok(CallToolResult::structured(json_value))
}

fn session_capabilities() -> Capabilities {
    Capabilities {
        supports_split_stdout_stderr: false,
        supports_exit_code: true,
        supports_resize: true,
    }
}

fn map_pty_error(error: PtyError) -> McpError {
    match error {
        PtyError::Api(api) => {
            let data = api_error_data(&api);
            match api.error_code {
                ErrorCode::InvalidArgument => McpError::invalid_params(api.message, data),
                ErrorCode::NotFound => McpError::resource_not_found(api.message, data),
                _ => McpError::internal_error(api.message, data),
            }
        }
        PtyError::Io(err) => {
            McpError::internal_error("IO error", Some(json!({"details": err.to_string()})))
        }
        PtyError::Json(err) => {
            McpError::invalid_params("Invalid JSON", Some(json!({"details": err.to_string()})))
        }
        PtyError::Regex(err) => {
            McpError::invalid_params("Invalid regex", Some(json!({"details": err.to_string()})))
        }
        PtyError::Timeout => McpError::internal_error("Timeout", None),
    }
}

fn api_error_data(error: &ApiError) -> Option<Value> {
    let mut data = serde_json::Map::new();
    data.insert(
        "code".to_string(),
        Value::String(error.error_code.to_string()),
    );
    if let Some(details) = &error.details {
        data.insert("details".to_string(), Value::String(details.clone()));
    }
    Some(Value::Object(data))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ControlRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ControlRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlRpcError>,
}

impl ControlRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, error: PtyError) -> Self {
        let api_error = match error {
            PtyError::Api(err) => err,
            PtyError::Io(err) => ApiError::new(ErrorCode::IoError, err.to_string()),
            PtyError::Json(err) => ApiError::new(ErrorCode::InvalidArgument, err.to_string()),
            PtyError::Regex(err) => ApiError::new(ErrorCode::InvalidArgument, err.to_string()),
            PtyError::Timeout => ApiError::new(ErrorCode::ExecTimeout, "Timeout"),
        };
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(ControlRpcError {
                code: -32000,
                message: api_error.message.clone(),
                data: Some(api_error),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ControlRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ApiError>,
}

fn decode_payload(data: &str, encoding: Encoding) -> PtyResult<Vec<u8>> {
    match encoding {
        Encoding::Utf8 => Ok(data.as_bytes().to_vec()),
        Encoding::Base64 => {
            use base64::Engine;
            use base64::engine::general_purpose::STANDARD;
            STANDARD.decode(data.as_bytes()).map_err(|err| {
                ApiError::new(ErrorCode::InvalidArgument, "Invalid base64")
                    .with_details(err.to_string())
                    .into()
            })
        }
    }
}

fn compile_input_hints(hints: Option<InputHints>) -> PtyResult<Option<Vec<Regex>>> {
    match hints.and_then(|h| h.wait_for_regexes) {
        Some(patterns) => {
            let mut compiled = Vec::new();
            for pattern in patterns {
                compiled.push(Regex::new(&pattern)?);
            }
            Ok(Some(compiled))
        }
        None => Ok(None),
    }
}

fn extract_exit_code(
    output: &str,
    marker_prefix: &str,
    marker_suffix: &str,
    fallback_marker: Option<&(String, String)>,
) -> (String, Option<i32>, Option<String>) {
    let primary_regex = Regex::new(&format!(
        "{}(?P<rc>\\d+){}",
        regex::escape(marker_prefix),
        regex::escape(marker_suffix)
    ))
    .ok();

    let fallback_regex = fallback_marker.and_then(|(prefix, suffix)| {
        Regex::new(&format!(
            "{}(?P<rc>\\d+){}",
            regex::escape(prefix),
            regex::escape(suffix)
        ))
        .ok()
    });

    let mut exit_code = None;
    if let Some(regex) = &primary_regex
        && let Some(caps) = regex.captures(output)
            && let Some(rc) = caps.name("rc").and_then(|m| m.as_str().parse::<i32>().ok()) {
                exit_code = Some(rc);
            }
    if exit_code.is_none()
        && let Some(regex) = &fallback_regex
            && let Some(caps) = regex.captures(output)
                && let Some(rc) = caps.name("rc").and_then(|m| m.as_str().parse::<i32>().ok()) {
                    exit_code = Some(rc);
                }

    let mut cleaned = output.to_string();
    if let Some(regex) = &primary_regex {
        cleaned = regex.replace_all(&cleaned, "").to_string();
    }
    if let Some(regex) = &fallback_regex {
        cleaned = regex.replace_all(&cleaned, "").to_string();
    }

    match exit_code {
        Some(rc) => (cleaned, Some(rc), None),
        None => (cleaned, None, Some("marker_not_seen".to_string())),
    }
}

fn build_marker_regex(
    marker_prefix: &str,
    marker_suffix: &str,
    fallback_marker: Option<&(String, String)>,
) -> PtyResult<Regex> {
    let primary = format!(
        "{}(\\d+){}",
        regex::escape(marker_prefix),
        regex::escape(marker_suffix)
    );
    if let Some((prefix, suffix)) = fallback_marker {
        let fallback = format!("{}(\\d+){}", regex::escape(prefix), regex::escape(suffix));
        Ok(Regex::new(&format!("(?:{})|(?:{})", primary, fallback))?)
    } else {
        Ok(Regex::new(&primary)?)
    }
}

fn extract_error_hints(output: &str, error_regexes: &[Regex]) -> Vec<String> {
    let mut hints = Vec::new();
    for regex in error_regexes {
        if regex.is_match(output) {
            hints.push(regex.as_str().to_string());
        }
    }
    hints
}

pub async fn serve_control_socket(
    server: McpServer,
    socket_path: &str,
    mode: ControlMode,
) -> PtyResult<()> {
    if std::path::Path::new(socket_path).exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path).map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Failed to bind control socket")
            .with_details(err.to_string())
    })?;

    loop {
        let (stream, _) = listener.accept().await.map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to accept control socket")
                .with_details(err.to_string())
        })?;
        let server = server.clone();
        let mode = mode.clone();
        tokio::spawn(async move {
            let _ = handle_control_stream(stream, server, mode).await;
        });
    }
}

async fn handle_control_stream(
    stream: UnixStream,
    server: McpServer,
    mode: ControlMode,
) -> PtyResult<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).lines();
    while let Some(line) = reader.next_line().await.map_err(PtyError::Io)? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: ControlRpcRequest = serde_json::from_str(line)?;
        let response = server
            .handle_control_request_filtered(request, mode.clone())
            .await;
        let payload = serde_json::to_string(&response)?;
        writer
            .write_all(payload.as_bytes())
            .await
            .map_err(PtyError::Io)?;
        writer.write_all(b"\n").await.map_err(PtyError::Io)?;
        writer.flush().await.map_err(PtyError::Io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_exit_code_from_marker() {
        let output = "ok\n\x1eRC=3\x1f\n";
        let (cleaned, rc, reason) = extract_exit_code(output, "\u{001e}RC=", "\u{001f}", None);
        assert!(cleaned.contains("ok"));
        assert_eq!(rc, Some(3));
        assert!(reason.is_none());
    }

    #[test]
    fn extract_exit_code_from_fallback_marker() {
        let output = "ok\nPTYCTL_RC_abc=7:END_abc\n";
        let fallback = ("PTYCTL_RC_abc=".to_string(), ":END_abc".to_string());
        let (cleaned, rc, reason) =
            extract_exit_code(output, "\u{001e}RC=", "\u{001f}", Some(&fallback));
        assert!(cleaned.contains("ok"));
        assert_eq!(rc, Some(7));
        assert!(reason.is_none());
    }
}
