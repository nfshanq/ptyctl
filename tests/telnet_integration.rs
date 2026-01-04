use ptyctl::config::{SessionConfig, SshConfig, TelnetLineEnding};
use ptyctl::session::{
    ExpectConfig, Protocol, PtyOptions, ReadParams, SessionManager, SessionOpenRequest, Timeouts,
    read_from_session,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const IAC: u8 = 0xff;
const DO: u8 = 0xfd;
const WILL: u8 = 0xfb;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

#[tokio::test]
async fn telnet_negotiation_and_read() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket
            .write_all(&[IAC, DO, OPT_TTYPE, IAC, DO, OPT_NAWS])
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = socket.read(&mut buf).await.unwrap();
        let received = buf[..n].to_vec();
        socket.write_all(&[IAC, IAC, b'A', b'\n']).await.unwrap();
        received
    });

    let manager = SessionManager::new(
        SessionConfig::default(),
        SshConfig::default(),
        TelnetLineEnding::Cr,
    );
    let open = manager
        .open_session(SessionOpenRequest {
            protocol: Protocol::Telnet,
            host: "127.0.0.1".to_string(),
            port: Some(addr.port()),
            username: None,
            auth: None,
            pty: Some(PtyOptions {
                enabled: true,
                cols: 80,
                rows: 24,
                term: "xterm".to_string(),
            }),
            timeouts: Some(Timeouts {
                connect_timeout_ms: Some(5_000),
                idle_timeout_ms: None,
            }),
            ssh_options: None,
            expect: Some(ExpectConfig::default()),
            session_type: None,
            device_id: None,
            acquire_lock: None,
            lock_ttl_ms: None,
            task_id: None,
        })
        .await
        .unwrap();

    let session = manager.get_session(&open.session_id).await.unwrap();
    let read = read_from_session(
        &session,
        ReadParams {
            cursor: Some(0),
            timeout_ms: 2_000,
            max_bytes: 1024,
            until_regex: None,
            include_match: true,
            until_idle_ms: None,
            input_hints: None,
        },
    )
    .await
    .unwrap();

    assert!(read.slice.bytes.contains(&IAC));

    let received = server_task.await.unwrap();
    assert!(received.windows(3).any(|w| w == [IAC, WILL, OPT_TTYPE]));
    assert!(received.windows(3).any(|w| w == [IAC, WILL, OPT_NAWS]));
}
