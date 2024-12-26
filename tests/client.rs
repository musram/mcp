use mcp_rs::{
    client::{Client, ClientInfo},
    error::McpError,
    server::{McpServer, config::ServerConfig},
    transport::SseTransport,
};
use tokio::time::Duration;

#[tokio::test]
async fn test_sse_client() -> Result<(), McpError> {
    const SERVER_HOST: &str = "127.0.0.1";
    const SERVER_PORT: u16 = 3030;

    let server_config = ServerConfig::default();
    let mut server = McpServer::new(server_config).await;
    
    let server_handle = tokio::spawn(async move {
        server.run_sse_transport().await
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut client = Client::new();

    // Try to connect with retries
    let mut retries = 3;
    let mut connected = false;
    while retries > 0 && !connected {
        let transport = SseTransport::new_client(SERVER_HOST.to_string(), SERVER_PORT, 32);
        match client.connect(transport).await {
            Ok(_) => {
                connected = true;
                break;
            }
            Err(e) => {
                println!("Connection attempt failed: {:?}, retrying...", e);
                retries -= 1;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    if !connected {
        server_handle.abort();
        return Err(McpError::ConnectionClosed);
    }

    // Initialize client
    let result = client
        .initialize(ClientInfo {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        })
        .await?;

    assert!(result.capabilities.resources.is_some());

    // Cleanup
    client.shutdown().await?;
    server_handle.abort();

    Ok(())
}
