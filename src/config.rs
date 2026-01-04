use crate::error::{ApiError, ErrorCode, PtyResult};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Transport {
    #[default]
    Stdio,
    Http,
    Both,
}


#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ControlMode {
    Disabled,
    #[default]
    Readonly,
    Readwrite,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum HostKeyPolicy {
    #[default]
    Strict,
    AcceptNew,
    Disabled,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TelnetLineEnding {
    #[default]
    Cr,
    Crlf,
    Lf,
    PassThrough,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub server: ServerConfig,
    pub session: SessionConfig,
    pub ssh: SshConfig,
    pub telnet: TelnetConfig,
    pub logging: LoggingConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub transport: Transport,
    pub http: HttpConfig,
    pub control: ControlConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            transport: Transport::Stdio,
            http: HttpConfig::default(),
            control: ControlConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    pub listen: String,
    pub auth_token: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8765".to_string(),
            auth_token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    pub control_socket_path: String,
    pub control_mode: ControlMode,
    pub control_auth_token: String,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            control_socket_path: default_control_socket_path(),
            control_mode: ControlMode::Readonly,
            control_auth_token: String::new(),
        }
    }
}

fn default_control_socket_path() -> String {
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR")
        && is_dir(&dir) {
            return format!("{}/ptyctl.sock", dir);
        }

    let uid = unsafe { libc::geteuid() };
    let run_user_dir = format!("/run/user/{}", uid);
    if is_dir(&run_user_dir) {
        return format!("{}/ptyctl.sock", run_user_dir);
    }

    format!("/tmp/ptyctl-{}.sock", uid)
}

fn is_dir(path: &str) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub max_sessions: usize,
    pub idle_timeout_ms: u64,
    pub output_buffer_max_lines: usize,
    pub output_buffer_max_bytes: usize,
    pub record_tx_events: bool,
    pub default_exec_timeout_ms: u64,
    pub default_read_timeout_ms: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_sessions: 100,
            idle_timeout_ms: 0,
            output_buffer_max_lines: 20000,
            output_buffer_max_bytes: 2 * 1024 * 1024,
            record_tx_events: false,
            default_exec_timeout_ms: 60_000,
            default_read_timeout_ms: 2_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    pub openssh_path: String,
    pub use_openssh_config: bool,
    pub config_path: String,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_path: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            openssh_path: "ssh".to_string(),
            use_openssh_config: true,
            config_path: String::new(),
            host_key_policy: HostKeyPolicy::Strict,
            known_hosts_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelnetConfig {
    pub telnet_path: String,
    pub line_ending: TelnetLineEnding,
}

impl Default for TelnetConfig {
    fn default() -> Self {
        Self {
            telnet_path: "telnet".to_string(),
            line_ending: TelnetLineEnding::Cr,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "text".to_string(),
        }
    }
}

#[derive(Debug, Parser)]
#[command(author, version = crate::version::VERSION, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    Serve(ServeArgs),
    Mcp(ServeArgs),
    Sessions(ControlClientArgs),
    Tail(ControlTailArgs),
    Attach(ControlAttachArgs),
}

#[derive(Debug, Parser, Clone)]
pub struct ServeArgs {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub transport: Option<Transport>,
    #[arg(long)]
    pub http_listen: Option<String>,
    #[arg(long)]
    pub auth_token: Option<String>,
    #[arg(long)]
    pub control_socket: Option<String>,
    #[arg(long)]
    pub control_mode: Option<ControlMode>,
    #[arg(long)]
    pub log_level: Option<String>,
}

#[derive(Debug, Parser, Clone)]
pub struct ControlClientArgs {
    #[arg(long)]
    pub control_socket: Option<String>,
}

#[derive(Debug, Parser, Clone)]
pub struct ControlTailArgs {
    pub session_id: String,
    #[arg(long, default_value_t = 65536)]
    pub max_bytes: usize,
    #[arg(long)]
    pub max_lines: Option<usize>,
    #[arg(long)]
    pub encoding: Option<String>,
    #[arg(long)]
    pub control_socket: Option<String>,
}

#[derive(Debug, Parser, Clone)]
pub struct ControlAttachArgs {
    #[arg(value_name = "SESSION_ID")]
    pub session_id: Option<String>,
    #[arg(long, default_value_t = 65536)]
    pub max_bytes: usize,
    #[arg(long)]
    pub control_socket: Option<String>,
}

impl Config {
    pub fn load(args: &ServeArgs) -> PtyResult<Self> {
        let mut config = if let Some(path) = &args.config {
            Self::from_file(path)?
        } else if Path::new("ptyctl.toml").exists() {
            Self::from_file(Path::new("ptyctl.toml"))?
        } else {
            Self::default()
        };

        config.apply_env();
        config.apply_cli(args);
        Ok(config)
    }

    fn from_file(path: &Path) -> PtyResult<Self> {
        let content = fs::read_to_string(path).map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to read config file")
                .with_details(err.to_string())
        })?;
        let parsed: Self = toml::from_str(&content).map_err(|err| {
            ApiError::new(ErrorCode::InvalidArgument, "Failed to parse config file")
                .with_details(err.to_string())
        })?;
        Ok(parsed)
    }

    fn apply_env(&mut self) {
        if let Ok(value) = env::var("PTYCTL_TRANSPORT")
            && let Some(transport) = parse_transport(&value) {
                self.server.transport = transport;
            }
        if let Ok(value) = env::var("PTYCTL_HTTP_LISTEN") {
            self.server.http.listen = value;
        }
        if let Ok(value) = env::var("PTYCTL_LOG_LEVEL") {
            self.logging.level = value;
        }
        if let Ok(value) = env::var("PTYCTL_CONTROL_SOCKET") {
            self.server.control.control_socket_path = value;
        }
        if let Ok(value) = env::var("PTYCTL_CONTROL_MODE")
            && let Some(mode) = parse_control_mode(&value) {
                self.server.control.control_mode = mode;
            }
    }

    fn apply_cli(&mut self, args: &ServeArgs) {
        if let Some(transport) = &args.transport {
            self.server.transport = transport.clone();
        }
        if let Some(listen) = &args.http_listen {
            self.server.http.listen = listen.clone();
        }
        if let Some(token) = &args.auth_token {
            self.server.http.auth_token = token.clone();
        }
        if let Some(path) = &args.control_socket {
            self.server.control.control_socket_path = path.clone();
        }
        if let Some(mode) = &args.control_mode {
            self.server.control.control_mode = mode.clone();
        }
        if let Some(level) = &args.log_level {
            self.logging.level = level.clone();
        }
    }
}

fn parse_transport(value: &str) -> Option<Transport> {
    match value.to_ascii_lowercase().as_str() {
        "stdio" => Some(Transport::Stdio),
        "http" => Some(Transport::Http),
        "both" => Some(Transport::Both),
        _ => None,
    }
}

fn parse_control_mode(value: &str) -> Option<ControlMode> {
    match value.to_ascii_lowercase().as_str() {
        "disabled" => Some(ControlMode::Disabled),
        "readonly" => Some(ControlMode::Readonly),
        "readwrite" => Some(ControlMode::Readwrite),
        _ => None,
    }
}
