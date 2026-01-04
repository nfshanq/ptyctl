use clap::Parser;
use ptyctl::config::{self, Cli, Command, ControlAttachArgs, ControlClientArgs, ControlTailArgs};
use ptyctl::error::{ApiError, ErrorCode, PtyError, PtyResult};
use ptyctl::mcp::{McpServer, serve_control_socket};
use ptyctl::session::SessionManager;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) | Command::Mcp(args) => run_server(args).await?,
        Command::Sessions(args) => run_sessions(args).await?,
        Command::Tail(args) => run_tail(args).await?,
        Command::Attach(args) => run_attach(args).await?,
    }
    Ok(())
}

async fn run_server(args: config::ServeArgs) -> PtyResult<()> {
    let config = config::Config::load(&args)?;
    init_logging(&config.logging);

    let session_manager = SessionManager::new(
        config.session.clone(),
        config.ssh.clone(),
        config.telnet.line_ending.clone(),
    );
    let server = McpServer::new(session_manager.clone(), config.session.clone());

    if !matches!(
        config.server.control.control_mode,
        config::ControlMode::Disabled
    ) {
        let control_path = config.server.control.control_socket_path.clone();
        let mode = config.server.control.control_mode.clone();
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_control_socket(server_clone, &control_path, mode).await {
                tracing::error!(error = %err, "Control socket task failed");
            }
        });
    }

    match config.server.transport {
        config::Transport::Stdio => server.clone().serve_stdio().await,
        config::Transport::Http => {
            server
                .clone()
                .serve_http(&config.server.http.listen, &config.server.http.auth_token)
                .await
        }
        config::Transport::Both => {
            let server_clone = server.clone();
            let http_listen = config.server.http.listen.clone();
            let auth_token = config.server.http.auth_token.clone();
            let stdio_task = tokio::spawn(async move { server_clone.serve_stdio().await });
            let http_task =
                tokio::spawn(async move { server.serve_http(&http_listen, &auth_token).await });
            let (stdio_res, http_res) = tokio::join!(stdio_task, http_task);
            stdio_res.map_err(|err| {
                ApiError::new(ErrorCode::IoError, "STDIO task failed").with_details(err.to_string())
            })??;
            http_res.map_err(|err| {
                ApiError::new(ErrorCode::IoError, "HTTP task failed").with_details(err.to_string())
            })??;
            Ok(())
        }
    }
}

fn init_logging(logging: &config::LoggingConfig) {
    let filter = tracing_subscriber::EnvFilter::new(logging.level.clone());
    if logging.format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}

async fn run_sessions(args: ControlClientArgs) -> PtyResult<()> {
    let socket_path = args.control_socket.unwrap_or_else(default_control_socket);
    let response =
        control_request_or_exit(&socket_path, "ptyctl_session", json!({ "action": "list" }))
            .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).unwrap_or_default()
    );
    Ok(())
}

async fn run_tail(args: ControlTailArgs) -> PtyResult<()> {
    let socket_path = args.control_socket.unwrap_or_else(default_control_socket);
    let encoding = args.encoding.unwrap_or_else(|| "utf-8".to_string());
    let response = control_request(
        &socket_path,
        "ptyctl_session_io",
        json!({
            "action": "read",
            "mode": "tail",
            "session_id": args.session_id,
            "max_bytes": args.max_bytes,
            "max_lines": args.max_lines,
            "encoding": encoding,
        }),
    )
    .await?;
    if let Some(chunk) = response.get("chunk").and_then(|v| v.as_str()) {
        print!("{chunk}");
    }
    Ok(())
}

async fn run_attach(args: ControlAttachArgs) -> PtyResult<()> {
    let socket_path = args.control_socket.unwrap_or_else(default_control_socket);
    let session_id = match args.session_id.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => {
            report_missing_session_id(&socket_path).await?;
            std::process::exit(2);
        }
    };
    let tail_response = control_request_or_exit(
        &socket_path,
        "ptyctl_session_io",
        json!({
            "action": "read",
            "mode": "tail",
            "session_id": session_id.as_str(),
            "max_bytes": args.max_bytes,
            "encoding": "utf-8"
        }),
    )
    .await?;
    let mut cursor = tail_response
        .get("next_cursor")
        .and_then(|v| v.as_str())
        .unwrap_or("0")
        .to_string();
    if let Some(chunk) = tail_response.get("chunk").and_then(|v| v.as_str()) {
        print!("{chunk}");
    }
    loop {
        let response = control_request_or_exit(
            &socket_path,
            "ptyctl_session_io",
            json!({
                "action": "read",
                "session_id": session_id.as_str(),
                "cursor": cursor,
                "timeout_ms": 2000,
                "max_bytes": args.max_bytes,
                "encoding": "utf-8"
            }),
        )
        .await?;
        if let Some(chunk) = response.get("chunk").and_then(|v| v.as_str())
            && !chunk.is_empty() {
                print!("{chunk}");
            }
        if let Some(next_cursor) = response.get("next_cursor").and_then(|v| v.as_str()) {
            cursor = next_cursor.to_string();
        }
    }
}

#[derive(Serialize)]
struct ControlRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct ControlResponse {
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ControlErrorPayload {
    message: String,
    data: Option<ApiError>,
}

async fn control_request(
    socket_path: &str,
    method: &str,
    params: serde_json::Value,
) -> PtyResult<serde_json::Value> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to connect control socket")
                .with_details(err.to_string())
        })?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader).lines();
    let request = ControlRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };
    let payload = serde_json::to_string(&request)?;
    writer.write_all(payload.as_bytes()).await.map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Control write failed").with_details(err.to_string())
    })?;
    writer.write_all(b"\n").await.map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Control write failed").with_details(err.to_string())
    })?;
    writer.flush().await.map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Control write failed").with_details(err.to_string())
    })?;

    if let Some(line) = reader.next_line().await.map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Control read failed").with_details(err.to_string())
    })? {
        let response: ControlResponse = serde_json::from_str(&line)?;
        if let Some(err) = response.error {
            let api_error = parse_control_error(err);
            return Err(api_error.into());
        }
        return response
            .result
            .ok_or_else(|| ApiError::new(ErrorCode::IoError, "Missing control response").into());
    }
    Err(ApiError::new(ErrorCode::IoError, "No response from control socket").into())
}

fn default_control_socket() -> String {
    config::Config::default().server.control.control_socket_path
}

async fn report_missing_session_id(socket_path: &str) -> PtyResult<()> {
    eprintln!("Missing session id.");
    match list_session_ids(socket_path).await {
        Ok(ids) => {
            if ids.is_empty() {
                eprintln!("No sessions are available.");
            } else {
                eprintln!("Available session ids:");
                for id in ids {
                    eprintln!("  {id}");
                }
                eprintln!("Usage: ptyctl attach <SESSION_ID>");
            }
            Ok(())
        }
        Err(err) => {
            if is_control_socket_connect_error(&err) {
                print_control_socket_hint(socket_path, &err);
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

async fn list_session_ids(socket_path: &str) -> PtyResult<Vec<String>> {
    let response =
        control_request(socket_path, "ptyctl_session", json!({ "action": "list" })).await?;
    let mut ids = Vec::new();
    if let Some(entries) = response.get("sessions").and_then(|value| value.as_array()) {
        for entry in entries {
            if let Some(session_id) = entry.get("session_id").and_then(|value| value.as_str()) {
                ids.push(session_id.to_string());
            }
        }
    }
    Ok(ids)
}

async fn control_request_or_exit(
    socket_path: &str,
    method: &str,
    params: serde_json::Value,
) -> PtyResult<serde_json::Value> {
    match control_request(socket_path, method, params).await {
        Ok(response) => Ok(response),
        Err(err) => {
            if is_control_socket_connect_error(&err) {
                print_control_socket_hint(socket_path, &err);
                std::process::exit(1);
            }
            if is_session_not_found_error(&err) {
                print_session_not_found_hint();
                std::process::exit(2);
            }
            Err(err)
        }
    }
}

fn is_control_socket_connect_error(err: &PtyError) -> bool {
    matches!(
        err,
        PtyError::Api(api)
            if api.error_code == ErrorCode::IoError && api.message == "Failed to connect control socket"
    )
}

fn is_session_not_found_error(err: &PtyError) -> bool {
    matches!(
        err,
        PtyError::Api(api) if api.error_code == ErrorCode::NotFound
    )
}

fn print_control_socket_hint(socket_path: &str, err: &PtyError) {
    eprintln!("Failed to connect control socket: {socket_path}");
    eprintln!(
        "Confirm the service is running (`ptyctl serve`), or use `--control-socket` / `PTYCTL_CONTROL_SOCKET` to point at the correct socket."
    );
    eprintln!("Use `ls -l {socket_path}` to check whether the socket exists and its permissions.");
    if let PtyError::Api(api) = err
        && let Some(details) = &api.details {
            eprintln!("Details: {details}");
        }
}

fn print_session_not_found_hint() {
    eprintln!("Session not found or already closed.");
    eprintln!("Use `ptyctl sessions` to list active sessions.");
}

fn parse_control_error(err: serde_json::Value) -> ApiError {
    if let Ok(payload) = serde_json::from_value::<ControlErrorPayload>(err.clone()) {
        if let Some(api) = payload.data {
            return api;
        }
        return ApiError::new(ErrorCode::IoError, payload.message);
    }
    ApiError::new(ErrorCode::IoError, "Control request failed").with_details(err.to_string())
}
