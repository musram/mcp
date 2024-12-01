use mcp_rs::{
    error::McpError, prompts::{Prompt, PromptCapabilities}, protocol::JsonRpcNotification, resource::{FileSystemProvider, ResourceCapabilities}, server::{config::{LoggingSettings, ResourceSettings, SecuritySettings, ServerConfig, ServerSettings, ToolSettings, TransportType}, McpServer}, NotificationSender
};
use tokio::sync::mpsc;
use std::{sync::Arc, time::Duration};
use tempfile::TempDir;

async fn setup_test_server(notif_tx: mpsc::Sender<JsonRpcNotification>) -> (Arc<McpServer>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    
    let config = ServerConfig {
        server: ServerSettings {
            name: "test-server".to_string(),
            version: "0.1.0".to_string(),
            transport: TransportType::Stdio,
            host: "127.0.0.1".to_string(),
            port: 0,
            max_connections: 10,
            timeout_ms: 1000,
        },
        resources: ResourceSettings {
            root_path: temp_dir.path().to_path_buf(),
            allowed_schemes: vec!["file".to_string()],
            max_file_size: 1024 * 1024,
            enable_templates: true,
        },
        security: SecuritySettings::default(),
        logging: LoggingSettings::default(),
        tools: ToolSettings::default(),
    };

    let mut server = McpServer::new(config);
    
    // Set up notification sender for all managers
    let notification_sender = NotificationSender {
        tx: notif_tx,
    };

    // Get mutable access to managers through Arc
    if let Some(resource_manager) = Arc::get_mut(&mut server.resource_manager) {
        resource_manager.set_notification_sender(notification_sender.clone());
    }
    if let Some(prompt_manager) = Arc::get_mut(&mut server.prompt_manager) {
        prompt_manager.set_notification_sender(notification_sender.clone());
    }
    
    let server = Arc::new(server);
    
    // Register file system provider
    let provider = Arc::new(FileSystemProvider::new(temp_dir.path()));
    server.resource_manager.register_provider("file".to_string(), provider).await;

    (server, temp_dir)
}

#[tokio::test]
async fn test_resource_update_notification() -> Result<(), McpError> {
    let (notif_tx, mut notif_rx) = mpsc::channel(32);
    let (server, temp_dir) = setup_test_server(notif_tx).await;
    
    // Create a file to monitor
    let test_file = temp_dir.path().join("test.txt");
    std::fs::write(&test_file, "initial content").unwrap();
    
    // Subscribe to the resource
    let uri = format!("file://{}", test_file.display());
    server.resource_manager.subscribe("test-client".to_string(), uri.clone()).await?;

    // Trigger a resource update notification
    server.resource_manager.notify_resource_updated(&uri).await?;

    // Wait for notification
    let timeout = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(timeout);

    let notification = tokio::select! {
        Some(n) = notif_rx.recv() => Some(n),
        _ = timeout => None,
    };

    assert!(notification.is_some());
    assert_eq!(notification.unwrap().method, "notifications/resources/updated");
    
    Ok(())
}

#[tokio::test]
async fn test_list_changed_notification() -> Result<(), McpError> {
    let (notif_tx, mut notif_rx) = mpsc::channel(32);
    let (server, _temp_dir) = setup_test_server(notif_tx).await;

    // Trigger a list changed notification
    server.resource_manager.notify_list_changed().await?;

    // Wait for notification
    let timeout = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(timeout);

    let notification = tokio::select! {
        Some(n) = notif_rx.recv() => Some(n),
        _ = timeout => None,
    };

    assert!(notification.is_some());
    assert_eq!(notification.unwrap().method, "notifications/resources/list_changed");
    
    Ok(())
}

#[tokio::test]
async fn test_prompt_list_changed_notification() -> Result<(), McpError> {
    let (notif_tx, mut notif_rx) = mpsc::channel(32);
    let (server, _temp_dir) = setup_test_server(notif_tx).await;

    // Register a new prompt which should trigger a notification
    let prompt = Prompt {
        name: "test-prompt".to_string(),
        description: "Test prompt".to_string(),
        arguments: vec![],
    };
    server.prompt_manager.register_prompt(prompt).await;

    // Wait for notification
    let timeout = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(timeout);

    let notification = tokio::select! {
        Some(n) = notif_rx.recv() => Some(n),
        _ = timeout => None,
    };

    assert!(notification.is_some());
    assert_eq!(notification.unwrap().method, "notifications/prompts/list_changed");
    
    Ok(())
}

#[tokio::test]
async fn test_multiple_subscribers() -> Result<(), McpError> {
    let (notif_tx, mut notif_rx) = mpsc::channel(32);
    let (server, temp_dir) = setup_test_server(notif_tx).await;
    
    // Create a file to monitor
    let test_file = temp_dir.path().join("test.txt");
    std::fs::write(&test_file, "initial content").unwrap();
    
    let uri = format!("file://{}", test_file.display());

    // Subscribe multiple clients
    server.resource_manager.subscribe("client1".to_string(), uri.clone()).await?;
    server.resource_manager.subscribe("client2".to_string(), uri.clone()).await?;

    // Trigger a resource update notification
    server.resource_manager.notify_resource_updated(&uri).await?;

    // Wait for notification
    let timeout = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(timeout);

    let notification = tokio::select! {
        Some(n) = notif_rx.recv() => Some(n),
        _ = timeout => None,
    };

    assert!(notification.is_some());
    assert_eq!(notification.unwrap().method, "notifications/resources/updated");
    
    Ok(())
}
