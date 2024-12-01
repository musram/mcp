use async_trait::async_trait;
use futures::StreamExt;
use jsonrpc_core::request;
use reqwest::RequestBuilder;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt}, sync::{broadcast, mpsc}};
use warp::Filter;
use std::{net::IpAddr, sync::{atomic::{AtomicU64, Ordering}, Arc}};

use crate::{error::McpError, protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse}};

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
    pub fn new(buffer_size: usize) -> Self {
        Self { buffer_size }
    }

    async fn run(
        mut reader: tokio::io::BufReader<tokio::io::Stdin>,
        writer: tokio::io::Stdout,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(32);
        
        // Spawn writer task
        let writer_handle = {
            let mut writer = writer;
            tokio::spawn(async move {
                while let Some(msg) = write_rx.recv().await {
                    if let Err(e) = writer.write_all(msg.as_bytes()).await {
                        tracing::error!("Error writing to stdout: {:?}", e);
                        break;
                    }
                }
            })
        };

        // Spawn reader task
        let reader_handle = {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => {
                            let _ = event_tx.send(TransportEvent::Closed).await;
                            break;
                        }
                        Ok(_) => {
                            match serde_json::from_str::<JsonRpcMessage>(&line) {
                                Ok(msg) => {
                                    if event_tx.send(TransportEvent::Message(msg)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Error parsing message: {:?}", e);
                                    if event_tx.send(TransportEvent::Error(McpError::ParseError)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error reading from stdin: {:?}", e);
                            let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
                            break;
                        }
                    }
                }
            })
        };

        // Main command loop
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    let msg_str = match serde_json::to_string(&msg) {
                        Ok(s) => s + "\n",
                        Err(_) => continue,
                    };
                    if write_tx.send(msg_str).await.is_err() {
                        break;
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

        let reader = tokio::io::BufReader::new(tokio::io::stdin());
        let writer = tokio::io::stdout();

        // Spawn the transport actor
        tokio::spawn(Self::run(reader, writer, cmd_rx, event_tx));

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
        // Create channels for client message broadcasting
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(100);
        let broadcast_tx = Arc::new(broadcast_tx);
        let broadcast_tx2 = broadcast_tx.clone();
        // Create a unique client ID generator
        let client_counter = Arc::new(AtomicU64::new(0));

        let host_clone = host.clone();

        // SSE route
        let sse_route = warp::path("sse")
            .and(warp::get())
            .map(move || {
                let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
            
                let broadcast_rx = broadcast_tx.subscribe();
                let endpoint = format!("http://{}:{}/message/{}", host.clone(), port, client_id);

                warp::sse::reply(warp::sse::keep_alive().stream(async_stream::stream! {
                    // Send initial endpoint event
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
            .map(move |client_id: u64, message: JsonRpcMessage| {
                let event_tx = event_tx.clone();
                tokio::spawn(async move {
                    let _ = event_tx.send(TransportEvent::Message(message)).await;
                });
                warp::reply()
            });

        // Combine routes
        let routes = sse_route.or(message_route);

        // Create command handler

        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        let _ = broadcast_tx2.send(msg);
                    }
                    TransportCommand::Close => break,
                }
            }
        });

        // Start server
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

       
        let rb = client.get(&sse_url);
        // Connect to SSE stream
        let mut sse = match EventSource::new(rb) {
            Ok(es) => es,
            Err(e) => {
                let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                return;
            }
        };

        // Wait for endpoint event
        let endpoint = loop {
            match sse.next().await {
                Some(Ok(Event::Message(m))) if m.event == "endpoint" => {
                    let endpoint: EndpointEvent = serde_json::from_str(m.data.as_str()).unwrap();
                    break endpoint.endpoint;
                }
                Some(Err(_)) => {
                    let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                    return;
                }
                None => {
                    let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                    return;
                }
                _ => continue,
            }
        };

        // Spawn SSE message handler
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            while let Some(Ok(event)) = sse.next().await {
               match event {
                    Event::Message(m) if m.event == "message" => {
                        let msg: JsonRpcMessage = serde_json::from_str(m.data.as_str()).unwrap();
                        let _ = event_tx_clone.send(TransportEvent::Message(msg)).await;
                    }
                    _ => continue,
                }
            }
        });

        // Handle outgoing messages
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    if let Err(_) = client.post(&endpoint)
                        .json(&msg)
                        .send()
                        .await {
                        let _ = event_tx.send(TransportEvent::Error(McpError::ConnectionClosed)).await;
                    }
                }
                TransportCommand::Close => break,
            }
        }

        // Cleanup
        let _ = event_tx.send(TransportEvent::Closed).await;
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
    use crate::{error::McpError, protocol::JsonRpcNotification, transport::{JsonRpcMessage, StdioTransport, Transport, TransportChannels, TransportCommand, TransportEvent}};

    #[tokio::test]
    async fn test_transport() ->Result<(), McpError> {
          // Create and start transport
    let mut transport = StdioTransport::new(32);
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
    cmd_tx.send(TransportCommand::SendMessage(JsonRpcMessage::Notification(
        JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "test".to_string(),
            params: None,
        }
    ))).await.unwrap();

    Ok(())
    }
}