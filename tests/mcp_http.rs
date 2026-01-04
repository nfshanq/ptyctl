use rmcp::model::CallToolRequestParam;
use rmcp::service::ServiceExt;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use std::net::{SocketAddr, TcpListener};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{Duration, Instant, sleep};

#[tokio::test]
async fn http_supports_initialize_list_tools_and_call() -> Result<(), Box<dyn std::error::Error>> {
    let port = pick_unused_port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let bin = env!("CARGO_BIN_EXE_ptyctl");

    let mut child = Command::new(bin)
        .arg("serve")
        .arg("--transport")
        .arg("http")
        .arg("--http-listen")
        .arg(addr.to_string())
        .arg("--control-mode")
        .arg("disabled")
        .spawn()?;

    let test_result: Result<(), Box<dyn std::error::Error>> = async {
        wait_for_port(addr).await?;

        let url = format!("http://{}/mcp", addr);
        let transport = StreamableHttpClientTransport::from_uri(url);
        let service = ().serve(transport).await?;

        let tools = service.list_tools(None).await?;
        let tool_names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert!(
            tool_names.contains(&"ptyctl_session"),
            "tools/list should include ptyctl_session"
        );

        let result = service
            .call_tool(CallToolRequestParam {
                name: "ptyctl_session".into(),
                arguments: Some(
                    serde_json::json!({
                        "action": "list"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            })
            .await?;

        let structured = result
            .structured_content
            .expect("session_list should return structured content");
        let sessions = structured
            .get("sessions")
            .and_then(|value| value.as_array())
            .expect("session_list should include sessions array");
        assert!(sessions.is_empty(), "session_list should start empty");

        service.cancel().await?;
        Ok(())
    }
    .await;

    let _ = child.kill().await;
    let _ = child.wait().await;

    test_result
}

#[tokio::test]
async fn http_rejects_missing_auth_token() -> Result<(), Box<dyn std::error::Error>> {
    let port = pick_unused_port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let bin = env!("CARGO_BIN_EXE_ptyctl");
    let auth_token = "test-token";

    let mut child = Command::new(bin)
        .arg("serve")
        .arg("--transport")
        .arg("http")
        .arg("--http-listen")
        .arg(addr.to_string())
        .arg("--auth-token")
        .arg(auth_token)
        .arg("--control-mode")
        .arg("disabled")
        .spawn()?;

    let test_result: Result<(), Box<dyn std::error::Error>> = async {
        wait_for_port(addr).await?;

        let url = format!("http://{}/mcp", addr);
        let transport = StreamableHttpClientTransport::from_uri(url.as_str());
        let unauthorized = ().serve(transport).await;
        assert!(unauthorized.is_err(), "expected auth to be required");

        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url.as_str()).auth_header(auth_token),
        );
        let service = ().serve(transport).await?;
        let tools = service.list_tools(None).await?;
        assert!(
            tools.tools.iter().any(|tool| tool.name == "ptyctl_session"),
            "authorized request should list tools"
        );
        service.cancel().await?;
        Ok(())
    }
    .await;

    let _ = child.kill().await;
    let _ = child.wait().await;

    test_result
}

fn pick_unused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to ephemeral port");
    listener.local_addr().expect("get local addr").port()
}

async fn wait_for_port(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("timeout waiting for HTTP server".into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}
