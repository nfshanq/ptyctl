use rmcp::model::{CallToolRequestParam, ErrorCode as McpErrorCode};
use rmcp::service::{ServiceError, ServiceExt};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

#[tokio::test]
async fn stdio_supports_initialize_list_tools_and_call() -> Result<(), Box<dyn std::error::Error>> {
    let bin = env!("CARGO_BIN_EXE_ptyctl");
    let transport = TokioChildProcess::new(Command::new(bin).configure(|cmd| {
        cmd.arg("serve")
            .arg("--transport")
            .arg("stdio")
            .arg("--control-mode")
            .arg("disabled");
    }))?;

    let service = ().serve(transport).await?;
    let tools = service.list_tools(None).await?;
    let tool_names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(
        tool_names.contains(&"ptyctl_session"),
        "tools/list should include ptyctl_session"
    );
    assert!(
        tool_names.contains(&"ptyctl_session_io"),
        "tools/list should include ptyctl_session_io"
    );
    assert!(
        tool_names.contains(&"ptyctl_session_exec"),
        "tools/list should include ptyctl_session_exec"
    );
    assert!(
        tool_names.contains(&"ptyctl_session_config"),
        "tools/list should include ptyctl_session_config"
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

#[tokio::test]
async fn stdio_reports_not_found_for_unknown_session() -> Result<(), Box<dyn std::error::Error>> {
    let bin = env!("CARGO_BIN_EXE_ptyctl");
    let transport = TokioChildProcess::new(Command::new(bin).configure(|cmd| {
        cmd.arg("serve")
            .arg("--transport")
            .arg("stdio")
            .arg("--control-mode")
            .arg("disabled");
    }))?;

    let service = ().serve(transport).await?;

    let result = service
        .call_tool(CallToolRequestParam {
            name: "ptyctl_session_io".into(),
            arguments: Some(
                serde_json::json!({
                    "action": "read",
                    "session_id": "missing-session",
                    "timeout_ms": 10,
                    "max_bytes": 16
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await;

    let outcome: Result<(), Box<dyn std::error::Error>> = match result {
        Err(ServiceError::McpError(err)) => {
            assert_eq!(err.code, McpErrorCode::RESOURCE_NOT_FOUND);
            Ok(())
        }
        Ok(_) => Err("expected an error for an unknown session id".into()),
        Err(err) => Err(format!("unexpected error: {err}").into()),
    };

    let _ = service.cancel().await;
    outcome
}
