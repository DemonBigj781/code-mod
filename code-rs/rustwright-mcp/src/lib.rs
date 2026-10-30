mod actor;
mod config;
mod server;
mod shaping;
mod tools;
mod transport;

use rmcp::ServiceExt;
use server::BrowserServer;
use transport::LifecycleStdio;

/// Serve the pinned native Rustwright MCP implementation over process stdio.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    BrowserServer::new()?
        .serve(LifecycleStdio::new())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;
    use serde_json::json;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;

    use super::*;

    async fn send_frame(writer: &mut tokio::io::DuplexStream, frame: Value) {
        writer
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .expect("write client frame");
        writer.flush().await.expect("flush client frame");
    }

    async fn receive_frame(reader: &mut BufReader<tokio::io::DuplexStream>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("server response timeout")
            .expect("read server frame");
        assert!(
            line.ends_with('\n'),
            "server response must be newline framed"
        );
        serde_json::from_str(&line).expect("server response must be JSON")
    }

    #[tokio::test]
    async fn handshake_lists_tools_calls_close_and_shuts_down() {
        let (mut client_input, server_input) = tokio::io::duplex(64 * 1024);
        let (server_output, client_output) = tokio::io::duplex(64 * 1024);
        let mut client_output = BufReader::new(client_output);

        let server = tokio::spawn(async move {
            BrowserServer::new()
                .map_err(|error| error.to_string())?
                .serve(LifecycleStdio::from_io(server_input, server_output))
                .await
                .map_err(|error| error.to_string())?
                .waiting()
                .await
                .map_err(|error| error.to_string())
        });

        send_frame(
            &mut client_input,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "code-rustwright-test", "version": "0"}
                }
            }),
        )
        .await;
        let initialized = receive_frame(&mut client_output).await;
        assert_eq!(initialized["id"], 1);
        assert_eq!(
            initialized["result"]["serverInfo"]["name"],
            "rustwright-mcp"
        );

        send_frame(
            &mut client_input,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        send_frame(
            &mut client_input,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .await;
        let tools = receive_frame(&mut client_output).await;
        assert_eq!(tools["id"], 2);
        assert!(
            tools["result"]["tools"]
                .as_array()
                .expect("tools array")
                .iter()
                .any(|tool| tool["name"] == "browser_navigate")
        );

        send_frame(
            &mut client_input,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "browser_close", "arguments": {}}
            }),
        )
        .await;
        let closed = receive_frame(&mut client_output).await;
        assert_eq!(closed["id"], 3);
        assert_eq!(closed["result"]["isError"], false);

        drop(client_input);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server shutdown timeout")
            .expect("server task panicked")
            .expect("server shutdown failed");
    }
}
