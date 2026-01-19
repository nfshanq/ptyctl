use crate::config::SshConfig;
use crate::error::{ApiError, ErrorCode, PtyResult};
use crate::session::{OutputHandle, PtyOptions, SessionBackend, SshAuth, SshOptions};
use async_trait::async_trait;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tempfile::NamedTempFile;

pub struct SshBackend {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
    eof: Arc<AtomicBool>,
    _key_file: Arc<Mutex<Option<NamedTempFile>>>,
}

pub(super) struct SshConnectParams<'a> {
    pub session_id: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub username: Option<String>,
    pub auth: Option<SshAuth>,
    pub options: Option<SshOptions>,
    pub ssh_config: &'a SshConfig,
    pub connect_timeout_ms: u64,
    pub pty: PtyOptions,
    pub output: OutputHandle,
}

struct SshArgsConfig<'a> {
    host: &'a str,
    port: u16,
    username: Option<String>,
    auth: Option<&'a SshAuth>,
    options: Option<&'a SshOptions>,
    ssh_config: &'a SshConfig,
    key_path: Option<PathBuf>,
    connect_timeout_ms: u64,
}

impl SshBackend {
    pub async fn connect(params: SshConnectParams<'_>) -> PtyResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: params.pty.rows,
                cols: params.pty.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| {
                ApiError::new(ErrorCode::ConnectFailed, "Failed to allocate PTY")
                    .with_details(err.to_string())
            })?;

        let key_file = params
            .auth
            .as_ref()
            .and_then(|auth| auth.private_key_pem.as_ref())
            .map(|pem| write_temp_key(pem))
            .transpose()?;

        let mut cmd = CommandBuilder::new(&params.ssh_config.openssh_path);
        cmd.env("TERM", &params.pty.term);

        let mut args = build_ssh_args(SshArgsConfig {
            host: params.host,
            port: params.port,
            username: params.username,
            auth: params.auth.as_ref(),
            options: params.options.as_ref(),
            ssh_config: params.ssh_config,
            key_path: key_file.as_ref().map(|file| file.path().to_path_buf()),
            connect_timeout_ms: params.connect_timeout_ms,
        })?;
        if params.pty.enabled {
            args.push("-tt".to_string());
        } else {
            args.push("-T".to_string());
        }
        cmd.args(args);

        let child = pair.slave.spawn_command(cmd).map_err(|err| {
            ApiError::new(ErrorCode::ConnectFailed, "Failed to spawn ssh")
                .with_details(err.to_string())
        })?;

        let mut reader = pair.master.try_clone_reader().map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to clone PTY reader")
                .with_details(err.to_string())
        })?;
        let writer = pair.master.take_writer().map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to take PTY writer")
                .with_details(err.to_string())
        })?;

        let eof = Arc::new(AtomicBool::new(false));
        let eof_flag = eof.clone();
        let output_clone = params.output.clone();
        let session_id = params.session_id.to_string();
        thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        output_clone.append_output(&buffer[..n]);
                    }
                    Err(err) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "SSH PTY read failed"
                        );
                        break;
                    }
                }
            }
            eof_flag.store(true, Ordering::SeqCst);
            output_clone.append_output(b"");
        });

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(pair.master)),
            child: Arc::new(Mutex::new(child)),
            eof,
            _key_file: Arc::new(Mutex::new(key_file)),
        })
    }
}

#[async_trait]
impl SessionBackend for SshBackend {
    async fn write(&self, data: &[u8]) -> PtyResult<usize> {
        let data = data.to_vec();
        let writer = self.writer.clone();

        tokio::task::spawn_blocking(move || -> PtyResult<usize> {
            let mut writer = writer.lock().expect("writer mutex poisoned");
            writer.write(&data).map_err(|err| {
                ApiError::new(ErrorCode::IoError, "Failed to write")
                    .with_details(err.to_string())
                    .into()
            })
        })
        .await
        .map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to join write").with_details(err.to_string())
        })?
    }

    async fn resize(&self, cols: u16, rows: u16) -> PtyResult<()> {
        let master = self.master.clone();

        tokio::task::spawn_blocking(move || -> PtyResult<()> {
            let master = master.lock().expect("master mutex poisoned");
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|err| {
                    ApiError::new(ErrorCode::IoError, "Failed to resize PTY")
                        .with_details(err.to_string())
                        .into()
                })
        })
        .await
        .map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to join resize").with_details(err.to_string())
        })?
    }

    async fn close(&self, _force: bool) -> PtyResult<()> {
        let child = self.child.clone();

        tokio::task::spawn_blocking(move || -> PtyResult<()> {
            let mut child = child.lock().expect("child mutex poisoned");
            child.kill().map_err(|err| {
                ApiError::new(ErrorCode::IoError, "Failed to kill ssh")
                    .with_details(err.to_string())
                    .into()
            })
        })
        .await
        .map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to join close").with_details(err.to_string())
        })?
    }

    fn is_eof(&self) -> bool {
        self.eof.load(Ordering::SeqCst)
    }
}

fn build_ssh_args(config: SshArgsConfig<'_>) -> PtyResult<Vec<String>> {
    let mut args = Vec::new();
    args.push("-p".to_string());
    args.push(config.port.to_string());

    if let Some(user) = config.username {
        args.push("-l".to_string());
        args.push(user);
    }

    let host_key_policy = config
        .options
        .and_then(|opts| opts.host_key_policy.clone())
        .unwrap_or_else(|| format!("{:?}", config.ssh_config.host_key_policy));
    match host_key_policy.to_ascii_lowercase().as_str() {
        "strict" => args.push("-o".to_string()),
        "acceptnew" | "accept_new" | "accept-new" => args.push("-o".to_string()),
        "disabled" => args.push("-o".to_string()),
        _ => args.push("-o".to_string()),
    }
    let policy_value = match host_key_policy.to_ascii_lowercase().as_str() {
        "acceptnew" | "accept_new" | "accept-new" => "StrictHostKeyChecking=accept-new",
        "disabled" => "StrictHostKeyChecking=no",
        _ => "StrictHostKeyChecking=yes",
    };
    args.push(policy_value.to_string());

    let known_hosts_path = config
        .options
        .and_then(|opts| opts.known_hosts_path.clone())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| config.ssh_config.known_hosts_path.clone());
    if !known_hosts_path.is_empty() {
        args.push("-o".to_string());
        args.push(format!("UserKnownHostsFile={}", known_hosts_path));
    }

    let use_config = config
        .options
        .and_then(|opts| opts.use_openssh_config)
        .unwrap_or(config.ssh_config.use_openssh_config);
    if !use_config {
        args.push("-F".to_string());
        args.push("/dev/null".to_string());
    } else {
        let config_path = config
            .options
            .and_then(|opts| opts.config_path.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| config.ssh_config.config_path.clone());
        if !config_path.is_empty() {
            args.push("-F".to_string());
            args.push(config_path);
        }
    }

    if let Some(auth) = config.auth {
        if auth.method.as_deref() == Some("agent") {
            args.push("-o".to_string());
            args.push("PreferredAuthentications=publickey".to_string());
        }
        if auth.method.as_deref() == Some("password") {
            args.push("-o".to_string());
            args.push("PreferredAuthentications=password,keyboard-interactive".to_string());
        }
    }

    if let Some(opts) = config.options {
        if let Some(extra) = &opts.extra_args {
            for arg in extra {
                args.push(arg.clone());
            }
        }
    }

    if config.connect_timeout_ms > 0 {
        args.push("-o".to_string());
        let seconds = (config.connect_timeout_ms as f64 / 1000.0).ceil() as u64;
        args.push(format!("ConnectTimeout={}", seconds.max(1)));
    }

    if let Some(path) = config.key_path {
        args.push("-i".to_string());
        args.push(path.to_string_lossy().to_string());
    }

    args.push(config.host.to_string());
    Ok(args)
}

fn write_temp_key(pem: &str) -> PtyResult<NamedTempFile> {
    let mut file = NamedTempFile::new().map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Failed to create temp key file")
            .with_details(err.to_string())
    })?;
    file.write_all(pem.as_bytes()).map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Failed to write private key")
            .with_details(err.to_string())
    })?;
    let mut perms = file
        .as_file()
        .metadata()
        .map_err(|err| {
            ApiError::new(ErrorCode::IoError, "Failed to read key file metadata")
                .with_details(err.to_string())
        })?
        .permissions();
    perms.set_mode(0o600);
    file.as_file().set_permissions(perms).map_err(|err| {
        ApiError::new(ErrorCode::IoError, "Failed to set key permissions")
            .with_details(err.to_string())
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ssh_args_basic() {
        let ssh_config = SshConfig::default();
        let args = build_ssh_args(SshArgsConfig {
            host: "example.com",
            port: 22,
            username: Some("root".to_string()),
            auth: None,
            options: None,
            ssh_config: &ssh_config,
            key_path: None,
            connect_timeout_ms: 15000,
        })
        .expect("args");
        assert!(args.contains(&"example.com".to_string()));
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"22".to_string()));
    }
}
