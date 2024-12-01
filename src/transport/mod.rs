use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt}, sync::mpsc};
use warp::Filter;
use std::sync::Arc;

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
pub struct SseTransport {
    port: u16,
    buffer_size: usize,
}

impl SseTransport {
    pub fn new(port: u16, buffer_size: usize) -> Self {
        Self { port, buffer_size }
    }

    async fn run(
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Create a channel for client connections
        let (client_tx, mut client_rx) = mpsc::channel::<mpsc::Sender<String>>(100);
        
        // Spawn HTTP server
        let server_handle = {
            let client_tx = client_tx.clone();
            tokio::spawn(async move {
                let routes = warp::path("events")
                    .and(warp::get())
                    .map(move || {
                        let (tx, mut rx) = mpsc::channel(32);
                        let _ = client_tx.try_send(tx);
                        
                        warp::sse::reply(warp::sse::keep_alive().stream(async_stream::stream! {
                            while let Some(msg) = rx.recv().await {
                                yield Ok::<_, warp::Error>(warp::sse::Event::default().data(msg));
                            }
                        }))
                    });

                warp::serve(routes).run(([127, 0, 0, 1], port)).await;
            })
        };

        // Maintain active clients
        let mut clients = Vec::new();

        // Main event loop
        loop {
            tokio::select! {
                // Handle new client connections
                Some(client) = client_rx.recv() => {
                    clients.push(client);
                }

                // Handle transport commands
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        TransportCommand::SendMessage(msg) => {
                            if let Ok(msg_str) = serde_json::to_string(&msg) {
                                clients.retain_mut(|client| {
                                    client.try_send(msg_str.clone()).is_ok()
                                });
                            }
                        }
                        TransportCommand::Close => break,
                    }
                }
            }
        }

        // Cleanup
        drop(server_handle);
        let _ = event_tx.send(TransportEvent::Closed).await;
    }
}

#[async_trait]
impl Transport for SseTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        tokio::spawn(Self::run(self.port, cmd_rx, event_tx));
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