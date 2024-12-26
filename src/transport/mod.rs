use async_trait::async_trait;
use futures::StreamExt;
use jsonrpc_core::request;
use reqwest::RequestBuilder;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    sync::{broadcast, mpsc},
};
use warp::Filter;

use crate::{
    error::McpError,
    protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
};

// Message types for the transport actor
#[derive(Debug)]
pub enum TransportCommand {
    SendMessage(JsonRpcMessage),
    Close,
}

#[derive(Debug)]
pub enum TransportEvent {
    Message(JsonRpcMessage),
    Error(McpError),
    Closed,
}

// JSON-RPC Message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

// Transport trait
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Start the transport and return channels for communication
    async fn start(&mut self) -> Result<TransportChannels, McpError>;
}

// Channels for communicating with the transport
#[derive(Debug, Clone)]
pub struct TransportChannels {
    /// Send commands to the transport
    pub cmd_tx: mpsc::Sender<TransportCommand>,
    /// Receive events from the transport
    pub event_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<TransportEvent>>>,
}

// Stdio Transport Implementation
pub struct StdioTransport {
    buffer_size: usize,
}

impl StdioTransport {
    pub fn new(buffer_size: Option<usize>) -> Self {
        // Only initialize logging if it hasn't been set up
        if std::env::var("RUST_LOG").is_ok() {
            // Try to initialize, but don't panic if it fails
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .try_init();
        }
        
        Self { buffer_size: buffer_size.unwrap_or(4092) }
    }

    pub async fn run(
        reader: tokio::io::BufReader<impl tokio::io::AsyncRead + Send + Unpin + 'static>,
        writer: impl tokio::io::AsyncWrite + Send + Unpin + 'static,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(32);

        // Writer task
        let writer_handle = {
            let mut writer = writer;
            tokio::spawn(async move {
                while let Some(msg) = write_rx.recv().await {
                    // Only log actual protocol messages, not transport logs
                    if !msg.contains("notifications/message") || !msg.contains("mcp_rs::transport") {
                        tracing::debug!("[Transport][Writer] >> {}", msg);
                    }

                    if let Err(e) = async {
                        writer.write_all(msg.as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        writer.flush().await?;
                        Ok::<_, std::io::Error>(())
                    }.await {
                        tracing::error!("[Transport][Writer] Write error: {:?}", e);
                        break;
                    }
                }
            })
        };

        // Reader task
        let reader_handle = tokio::spawn({
            let mut reader = reader;
            let event_tx = event_tx.clone();
            async move {
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let trimmed = line.trim();
                            // Skip lines that don't look like JSON
                            if !trimmed.is_empty() && trimmed.starts_with('{') {
                                match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                                    Ok(msg) => {
                                        if !should_skip_message(&msg) {
                                            let _ = event_tx.send(TransportEvent::Message(msg)).await;
                                        }
                                    }
                                    Err(e) => tracing::error!("[Transport][Reader] Parse error: {}", e),
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("[Transport][Reader] Read error: {:?}", e);
                            break;
                        }
                    }
                }
            }
        });

        // Main message loop
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    // Skip processing transport logging messages
                    if !should_skip_message(&msg) {
                        match serde_json::to_string(&msg) {
                            Ok(s) => {
                                if let Err(e) = write_tx.send(s).await {
                                    tracing::error!("[Transport] Failed to send to writer: {:?}", e);
                                    break;
                                }
                            }
                            Err(e) => tracing::error!("[Transport] Serialization error: {:?}", e),
                        }
                    }
                }
                TransportCommand::Close => break,
            }
        }

        // Cleanup
        drop(write_tx);
        let _ = reader_handle.await;
        let _ = writer_handle.await;
        let _ = event_tx.send(TransportEvent::Closed).await;
    }
}

// Helper function to determine if a message should be skipped
fn should_skip_message(msg: &JsonRpcMessage) -> bool {
    match msg {
        JsonRpcMessage::Notification(n) => {
            // Skip transport logging messages
            if n.method == "notifications/message" {
                if let Some(params) = &n.params {
                    if let Some(logger) = params.get("logger").and_then(|l| l.as_str()) {
                        return logger.contains("mcp_rs::transport");
                    }
                }
            }
            false
        }
        _ => false
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        // Set up buffered stdin/stdout
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let reader = tokio::io::BufReader::with_capacity(4096, stdin);

        // Spawn the transport actor
        tokio::spawn(Self::run(reader, stdout, cmd_rx, event_tx));

        let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));
        Ok(TransportChannels { cmd_tx, event_rx })
    }
}

// SSE Transport Implementation
#[derive(Debug, Serialize, Deserialize)]
struct EndpointEvent {
    endpoint: String,
}

pub struct SseTransport {
    host: String,
    port: u16,
    client_mode: bool,
    buffer_size: usize,
}

impl SseTransport {
    pub fn new_server(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: false,
            buffer_size,
        }
    }

    pub fn new_client(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: true,
            buffer_size,
        }
    }

    async fn run_server(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Create broadcast channel for SSE clients
        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<JsonRpcMessage>(100);
        let broadcast_tx = Arc::new(broadcast_tx);
        let broadcast_tx2 = Arc::clone(&broadcast_tx);

        // Client counter for unique IDs
        let client_counter = Arc::new(AtomicU64::new(0));
        let host_clone = host.clone();

        // SSE endpoint route
        let sse_route = warp::path("sse")
            .and(warp::get())
            .map(move || {
                let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
                let broadcast_rx = broadcast_tx.subscribe();
                let endpoint = format!("http://{}:{}/message/{}", host.clone(), port, client_id);

                warp::sse::reply(warp::sse::keep_alive()
                    .interval(Duration::from_secs(30))
                    .stream(async_stream::stream! {
                        yield Ok::<_, warp::Error>(warp::sse::Event::default()
                            .event("endpoint")
                            .json_data(&EndpointEvent { endpoint })
                            .unwrap());

                        let mut broadcast_rx = broadcast_rx;
                        while let Ok(msg) = broadcast_rx.recv().await {
                            yield Ok::<_, warp::Error>(warp::sse::Event::default()
                                .event("message")
                                .json_data(&msg)
                                .unwrap());
                        }
                    }))
            });

        // Message receiving route
        let message_route = warp::path!("message" / u64)
            .and(warp::post())
            .and(warp::body::json())
            .map(move |_client_id: u64, message: JsonRpcMessage| {
                let event_tx = event_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = event_tx.send(TransportEvent::Message(message)).await {
                        tracing::error!("Failed to forward message: {:?}", e);
                    }
                });
                warp::reply()
            });

        // Combine routes
        let routes = sse_route.or(message_route);

        // Message forwarding task
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        // Skip broadcasting debug log messages about SSE and internal operations
                        let should_skip = match &msg {
                            JsonRpcMessage::Notification(n) if n.method == "notifications/message" => {
                                if let Some(params) = &n.params {
                                    // Check the log message and logger
                                    let is_debug = params.get("level")
                                        .and_then(|l| l.as_str())
                                        .map_or(false, |l| l == "debug");
                                    
                                    let logger = params.get("logger")
                                        .and_then(|l| l.as_str())
                                        .unwrap_or("");

                                    let message = params.get("data")
                                        .and_then(|d| d.get("message"))
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("");

                                    is_debug && (
                                        logger.starts_with("hyper::") ||
                                        logger.starts_with("mcp_rs::transport") ||
                                        message.contains("Broadcasting SSE message") ||
                                        message.contains("Failed to broadcast message")
                                    )
                                } else {
                                    false
                                }
                            }
                            _ => false
                        };

                        if !should_skip {
                            tracing::debug!("Broadcasting SSE message: {:?}", msg);
                            if let Err(e) = broadcast_tx2.send(msg) {
                                tracing::error!("Failed to broadcast message: {:?}", e);
                            }
                        }
                    }
                    TransportCommand::Close => break,
                }
            }
        });

        // Start the server
        warp::serve(routes)
            .run((host_clone.parse::<IpAddr>().unwrap(), port))
            .await;
    }

    async fn run_client(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let client = reqwest::Client::new();
        let sse_url = format!("http://{}:{}/sse", host, port);

        tracing::debug!("Connecting to SSE endpoint: {}", sse_url);

        let rb = client.get(&sse_url);
        let mut sse = match EventSource::new(rb) {
            Ok(es) => es,
            Err(e) => {
                tracing::error!("Failed to create EventSource: {:?}", e);
                let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                return;
            }
        };

        // Wait for endpoint event
        let endpoint = match Self::wait_for_endpoint(&mut sse).await {
            Some(ep) => ep,
            None => {
                tracing::error!("Failed to receive endpoint");
                let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                return;
            }
        };

        tracing::debug!("Received message endpoint: {}", endpoint);

        // Message receiving task
        let event_tx2 = event_tx.clone();
        tokio::spawn(async move {
            while let Some(Ok(event)) = sse.next().await {
                match event {
                    Event::Message(m) if m.event == "message" => {
                        match serde_json::from_str::<JsonRpcMessage>(&m.data) {
                            Ok(msg) => {
                                if let Err(e) = event_tx2.send(TransportEvent::Message(msg)).await {
                                    tracing::error!("Failed to forward SSE message: {:?}", e);
                                    break;
                                }
                            }
                            Err(e) => tracing::error!("Failed to parse SSE message: {:?}", e),
                        }
                    }
                    _ => continue,
                }
            }
        });

        // Message sending task
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    tracing::debug!("Sending message to {}: {:?}", endpoint, msg);
                    if let Err(e) = client.post(&endpoint).json(&msg).send().await {
                        tracing::error!("Failed to send message: {:?}", e);
                    }
                }
                TransportCommand::Close => break,
            }
        }

        let _ = event_tx.send(TransportEvent::Closed).await;
    }

    async fn wait_for_endpoint(sse: &mut EventSource) -> Option<String> {
        while let Some(Ok(event)) = sse.next().await {
            if let Event::Message(m) = event {
                if m.event == "endpoint" {
                    if let Ok(endpoint) = serde_json::from_str::<EndpointEvent>(&m.data) {
                        return Some(endpoint.endpoint);
                    }
                }
            }
        }
        None
    }
}

#[async_trait]
impl Transport for SseTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        if self.client_mode {
            tokio::spawn(Self::run_client(
                self.host.clone(),
                self.port,
                cmd_rx,
                event_tx,
            ));
        } else {
            tokio::spawn(Self::run_server(
                self.host.clone(),
                self.port,
                cmd_rx,
                event_tx,
            ));
        }

        let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));

        Ok(TransportChannels { cmd_tx, event_rx })
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::{sync::Arc, time::Duration};
    use tokio::sync::mpsc;
    use crate::{
        error::McpError,
        protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
        transport::{
            JsonRpcMessage, StdioTransport, Transport, TransportChannels, TransportCommand,
            TransportEvent,
        },
    };
    use tokio::time::timeout;
    use serde_json::json;

    // Helper function to create a test message
    fn create_test_request() -> JsonRpcMessage {
        JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "test_method".to_string(),
            params: Some(json!({"test": "data"})),
        })
    }

    // Helper function to create a test response
    fn create_test_response() -> JsonRpcMessage {
        JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: 1,
            result: Some(json!({"status": "success"})),
            error: None,
        })
    }

    // Mock transport for testing
    struct MockTransport {
        rx: mpsc::Receiver<String>,
        tx: mpsc::Sender<String>,
    }

    impl MockTransport {
        fn new() -> (Self, mpsc::Sender<String>, mpsc::Receiver<String>) {
            let (input_tx, input_rx) = mpsc::channel(32);
            let (output_tx, output_rx) = mpsc::channel(32);
            (Self { rx: input_rx, tx: output_tx }, input_tx, output_rx)
        }
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn start(&mut self) -> Result<TransportChannels, McpError> {
            let (cmd_tx, mut cmd_rx) = mpsc::channel(32);
            let (event_tx, event_rx) = mpsc::channel(32);
            
            let mut rx = std::mem::replace(&mut self.rx, mpsc::channel(1).1);
            let tx = self.tx.clone();
            let event_tx2 = event_tx.clone();

            // Handle outgoing messages
            tokio::spawn(async move {
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        TransportCommand::SendMessage(msg) => {
                            let s = serde_json::to_string(&msg).unwrap();
                            // Echo the message back through the event channel
                            if let Ok(parsed) = serde_json::from_str::<JsonRpcMessage>(&s) {
                                let _ = event_tx2.send(TransportEvent::Message(parsed)).await;
                            }
                            let _ = tx.send(s).await;
                        }
                        TransportCommand::Close => break,
                    }
                }
            });

            // Handle incoming messages
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if let Ok(parsed) = serde_json::from_str::<JsonRpcMessage>(&msg) {
                        let _ = event_tx.send(TransportEvent::Message(parsed)).await;
                    }
                }
            });

            Ok(TransportChannels {
                cmd_tx,
                event_rx: Arc::new(tokio::sync::Mutex::new(event_rx)),
            })
        }
    }

    #[tokio::test]
    async fn test_transport_basic() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Send a test message
        let test_msg = create_test_request();
        cmd_tx.send(TransportCommand::SendMessage(test_msg.clone())).await?;

        // Verify output
        let output = output_rx.recv().await.expect("Should receive message");
        assert_eq!(output, serde_json::to_string(&test_msg)?);

        // Send response back
        let response = create_test_response();
        input_tx.send(serde_json::to_string(&response)?).await?;

        // Verify received
        let received = timeout(Duration::from_secs(1), async {
            let mut guard = event_rx.lock().await;
            guard.recv().await
        }).await?;

        assert!(matches!(received, Some(TransportEvent::Message(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_transport() -> Result<(), McpError> {
        // Create and start transport
        let mut transport = StdioTransport::new(None);
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Handle events
        tokio::spawn(async move {
            let event_rx = event_rx.clone();

            loop {
                let event = {
                    let mut guard = event_rx.lock().await;
                    guard.recv().await
                };

                match event {
                    Some(TransportEvent::Message(msg)) => println!("Received: {:?}", msg),
                    Some(TransportEvent::Error(err)) => println!("Error: {:?}", err),
                    Some(TransportEvent::Closed) => break,
                    None => break,
                }
            }
        });

        // Send a message
        cmd_tx
            .send(TransportCommand::SendMessage(JsonRpcMessage::Notification(
                JsonRpcNotification {
                    jsonrpc: "2.0".to_string(),
                    method: "test".to_string(),
                    params: None,
                },
            )))
            .await
            .unwrap();

        Ok(())
    }

    #[tokio::test]
    async fn test_transport_initialization_sequence() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Test initialization sequence
        let init_request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "initialize".to_string(),
            params: Some(json!({
                "name": "test-client",
                "version": "1.0.0"
            })),
        });

        cmd_tx.send(TransportCommand::SendMessage(init_request.clone())).await?;
        
        // Verify output
        let output = output_rx.recv().await.expect("Should receive message");
        assert_eq!(output, serde_json::to_string(&init_request)?);

        Ok(())
    }

    #[tokio::test]
    async fn test_transport_message_ordering() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Send multiple messages with different IDs
        let messages: Vec<JsonRpcMessage> = (1..=5).map(|id| {
            JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: id as u64,
                method: "test_method".to_string(),
                params: Some(json!({"sequence": id})),
            })
        }).collect();

        // Send all messages
        for msg in messages.clone() {
            cmd_tx.send(TransportCommand::SendMessage(msg)).await?;
        }

        // Verify messages are received in order
        let mut received_ids = Vec::new();
        for _ in 0..messages.len() {
            if let Ok(Some(TransportEvent::Message(msg))) = timeout(
                Duration::from_secs(1),
                event_rx.lock().await.recv()
            ).await {
                match msg {
                    JsonRpcMessage::Request(req) => {
                        received_ids.push(req.id);
                    }
                    _ => {}
                }
            }
        }

        assert_eq!(received_ids, vec![1, 2, 3, 4, 5], "Messages received out of order");
        Ok(())
    }

    #[tokio::test]
    async fn test_transport_notification_handling() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Send a notification (message without ID)
        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "test/notification".to_string(),
            params: Some(json!({"type": "test"})),
        });

        cmd_tx.send(TransportCommand::SendMessage(notification.clone())).await?;

        // Verify notification handling
        let received = timeout(
            Duration::from_secs(1),
            event_rx.lock().await.recv()
        ).await;

        assert!(matches!(
            received,
            Ok(Some(TransportEvent::Message(_)))
        ), "Notification not handled correctly");

        Ok(())
    }

    #[tokio::test]
    async fn test_transport_large_message() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Create a large message
        let large_data = (0..1000).map(|i| format!("data_{}", i)).collect::<Vec<_>>();
        let large_message = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "test_large".to_string(),
            params: Some(json!({
                "data": large_data
            })),
        });

        // Send large message
        cmd_tx.send(TransportCommand::SendMessage(large_message.clone())).await?;

        // Verify large message handling
        let received = timeout(
            Duration::from_secs(1),
            event_rx.lock().await.recv()
        ).await;

        assert!(matches!(
            received,
            Ok(Some(TransportEvent::Message(_)))
        ), "Large message not handled correctly");

        Ok(())
    }

    #[tokio::test]
    async fn test_transport_concurrent_messages() -> Result<(), McpError> {
        let (mut transport, input_tx, mut output_rx) = MockTransport::new();
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Create multiple message senders
        let mut handles = vec![];
        for i in 0..5 {
            let cmd_tx = cmd_tx.clone();
            let handle = tokio::spawn(async move {
                let msg = JsonRpcMessage::Request(JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: i as u64,
                    method: "test_concurrent".to_string(),
                    params: Some(json!({"sender": i})),
                });
                cmd_tx.send(TransportCommand::SendMessage(msg)).await
            });
            handles.push(handle);
        }

        // Wait for all sends to complete
        for handle in handles {
            handle.await.map_err(|e| McpError::InternalError(e.to_string()))??;
        }

        // Verify all messages were received
        let mut received_count = 0;
        while let Ok(Some(_)) = timeout(
            Duration::from_secs(1),
            event_rx.lock().await.recv()
        ).await {
            received_count += 1;
            if received_count == 5 {
                break;
            }
        }

        assert_eq!(received_count, 5, "Not all concurrent messages were received");
        Ok(())
    }
}