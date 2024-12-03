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
        Self { buffer_size: buffer_size.unwrap_or(4092) }
    }

    async fn run(
        reader: tokio::io::BufReader<tokio::io::Stdin>,
        writer: tokio::io::Stdout,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(32);

        // Writer task
        let writer_handle = {
            let mut writer = writer;
            tokio::spawn(async move {
                while let Some(msg) = write_rx.recv().await {
                    // Skip logging for certain types of messages
                    if !msg.contains("notifications/message") && !msg.contains("list_changed") {
                        tracing::debug!("-> {}", msg);
                    }

                    if let Err(e) = async {
                        writer.write_all(msg.as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        writer.flush().await?;
                        Ok::<_, std::io::Error>(())
                    }.await {
                        tracing::error!("Write error: {:?}", e);
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
                            if (!trimmed.contains("notifications/message") && !trimmed.contains("list_changed")) {
                                tracing::debug!("<- {}", trimmed);
                            }

                            if !trimmed.is_empty() {
                                match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                                    Ok(msg) => {
                                        if event_tx.send(TransportEvent::Message(msg)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Parse error: {}, input: {}", e, trimmed);
                                        if event_tx.send(TransportEvent::Error(McpError::ParseError)).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Read error: {:?}", e);
                            let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
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
                    match serde_json::to_string(&msg) {
                        Ok(s) => {
                            if write_tx.send(s).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::error!("Failed to serialize message: {:?}", e),
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
    use crate::{
        error::McpError,
        protocol::JsonRpcNotification,
        transport::{
            JsonRpcMessage, StdioTransport, Transport, TransportChannels, TransportCommand,
            TransportEvent,
        },
    };

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
}
