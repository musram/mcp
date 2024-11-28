use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::{error::McpError, resource::ResourceCapabilities, transport::{JsonRpcMessage, Transport, TransportChannels, TransportCommand, TransportEvent}};

// Constants
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 60000;

// Protocol Options
#[derive(Debug, Clone)]
pub struct ProtocolOptions {
    /// Whether to enforce strict capability checking
    pub enforce_strict_capabilities: bool,
}

impl Default for ProtocolOptions {
    fn default() -> Self {
        Self {
            enforce_strict_capabilities: false,
        }
    }
}

// Progress types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub progress: u64,
    pub total: Option<u64>,
}

pub type ProgressCallback = Box<dyn Fn(Progress) + Send + Sync>;


pub struct RequestOptions {
    pub on_progress: Option<ProgressCallback>,
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub timeout: Option<Duration>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            on_progress: None,
            signal: None,
            timeout: Some(Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS)),
        }
    }
}

// Request handler extra data
pub struct RequestHandlerExtra {
    pub signal: tokio::sync::watch::Receiver<bool>,
}

// Protocol implementation
pub struct Protocol {
    cmd_tx: Option<mpsc::Sender<TransportCommand>>,
    event_rx: Option<Arc<tokio::sync::Mutex<mpsc::Receiver<TransportEvent>>>>,
    options: ProtocolOptions,
    request_message_id: Arc<RwLock<u64>>,
    request_handlers: Arc<RwLock<HashMap<String, RequestHandler>>>,
    notification_handlers: Arc<RwLock<HashMap<String, NotificationHandler>>>,
    response_handlers: Arc<RwLock<HashMap<u64, ResponseHandler>>>,
    progress_handlers: Arc<RwLock<HashMap<u64, ProgressCallback>>>,
    request_abort_controllers: Arc<RwLock<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
}

type RequestHandler = Box<
    dyn Fn(JsonRpcRequest, RequestHandlerExtra) -> BoxFuture<Result<serde_json::Value, McpError>>
        + Send
        + Sync,
>;
type NotificationHandler =
    Box<dyn Fn(JsonRpcNotification) -> BoxFuture<Result<(), McpError>> + Send + Sync>;
type ResponseHandler = Box<dyn FnOnce(Result<JsonRpcResponse, McpError>) + Send + Sync>;
type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

impl Protocol {
    pub fn new(options: Option<ProtocolOptions>) -> Self {
        let options = options.unwrap_or_default();
        
        let mut protocol = Self {
            cmd_tx: None,
            event_rx: None,
            options,
            request_message_id: Arc::new(RwLock::new(0)),
            request_handlers: Arc::new(RwLock::new(HashMap::new())),
            notification_handlers: Arc::new(RwLock::new(HashMap::new())),
            response_handlers: Arc::new(RwLock::new(HashMap::new())),
            progress_handlers: Arc::new(RwLock::new(HashMap::new())),
            request_abort_controllers: Arc::new(RwLock::new(HashMap::new())),
        };

        protocol.register_default_handlers();
        protocol
    }

    pub async fn connect<T: Transport>(&mut self, mut transport: T) -> Result<(), McpError> {
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;
        let cmd_tx_clone = cmd_tx.clone();
        // Start message handling loop
        let event_rx_clone = Arc::clone(&event_rx);
        let request_handlers = Arc::clone(&self.request_handlers);
        let notification_handlers = Arc::clone(&self.notification_handlers);
        let response_handlers = Arc::clone(&self.response_handlers);
        let progress_handlers = Arc::clone(&self.progress_handlers);

        tokio::spawn(async move {
            loop {
                let event = {
                    let mut rx = event_rx_clone.lock().await;
                    rx.recv().await
                };

                match event {
                    Some(TransportEvent::Message(msg)) => match msg {
                        JsonRpcMessage::Request(req) => {
                            let handlers = request_handlers.read().await;
                            if let Some(handler) = handlers.get(&req.method) {
                                // Create abort controller for the request
                                let (tx, rx) = tokio::sync::watch::channel(false);
                                let extra = RequestHandlerExtra { signal: rx };
                                
                                // Handle request
                                let result = handler(req.clone(), extra).await;
                                
                                // Send response
                                let response = match result {
                                    Ok(result) => JsonRpcMessage::Response(JsonRpcResponse {
                                        jsonrpc: "2.0".to_string(),
                                        id: req.id,
                                        result: Some(result),
                                        error: None,
                                    }),
                                    Err(e) => JsonRpcMessage::Response(JsonRpcResponse {
                                        jsonrpc: "2.0".to_string(),
                                        id: req.id,
                                        result: None,
                                        error: Some(JsonRpcError {
                                            code: e.code(),
                                            message: e.to_string(),
                                            data: None,
                                        }),
                                    }),
                                };
                                
                                let _ = cmd_tx.send(TransportCommand::SendMessage(response)).await;
                            }
                        }
                        JsonRpcMessage::Response(resp) => {
                            let mut handlers = response_handlers.write().await;
                            if let Some(handler) = handlers.remove(&resp.id) {
                                handler(Ok(resp));
                            }
                        }
                        JsonRpcMessage::Notification(notif) => {
                            let handlers = notification_handlers.read().await;
                            if let Some(handler) = handlers.get(&notif.method) {
                                let _ = handler(notif.clone()).await;
                            }
                        }
                    },
                    Some(TransportEvent::Error(e)) => {
                        // Handle transport error
                        // TODO: Implement error handling
                    }
                    Some(TransportEvent::Closed) => break,
                    None => break,
                }
            }
        });

        self.cmd_tx = Some(cmd_tx_clone);
        self.event_rx = Some(event_rx);
        
        Ok(())
    }

    pub async fn request<Req, Resp>(
        &self,
        method: &str,
        params: Option<Req>,
        options: Option<RequestOptions>,
    ) -> Result<Resp, McpError>
    where
        Req: Serialize,
        Resp: for<'de> Deserialize<'de>,
    {
        let options = options.unwrap_or_default();
        
        if self.options.enforce_strict_capabilities {
            self.assert_capability_for_method(method)?;
        }

        let message_id = {
            let mut id = self.request_message_id.write().await;
            *id += 1;
            *id
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        
        let mut params_value = serde_json::to_value(params).map_err(|_| McpError::InvalidParams)?;
        
        if let Some(progress_callback) = options.on_progress {
            self.progress_handlers.write().await.insert(message_id, progress_callback);
            
            if let serde_json::Value::Object(ref mut map) = params_value {
                map.insert(
                    "_meta".to_string(),
                    serde_json::json!({ "progressToken": message_id }),
                );
            }
        }

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: message_id,
            method: method.to_string(),
            params: Some(params_value),
        });

        self.response_handlers.write().await.insert(
            message_id,
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        );

        if let Some(cmd_tx) = &self.cmd_tx {
            cmd_tx.send(TransportCommand::SendMessage(request)).await
                .map_err(|_| McpError::ConnectionClosed)?;
        } else {
            return Err(McpError::NotConnected);
        }

        // Setup timeout
        let timeout = options.timeout.unwrap_or(Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS));

        tokio::select! {
            response = rx => {
                match response {
                    Ok(Ok(response)) => {
                        self.progress_handlers.write().await.remove(&message_id);
                        serde_json::from_value(response.result.unwrap_or_default())
                            .map_err(|_| McpError::InvalidParams)
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(McpError::InternalError),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                self.progress_handlers.write().await.remove(&message_id);
                Err(McpError::RequestTimeout)
            }
        }
    }

    pub async fn notification<N: Serialize>(
        &self,
        method: &str,
        params: Option<N>,
    ) -> Result<(), McpError> {
        self.assert_notification_capability(method)?;

        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params.map(|p| serde_json::to_value(p).unwrap()),
        });

        if let Some(cmd_tx) = &self.cmd_tx {
            cmd_tx.send(TransportCommand::SendMessage(notification)).await
                .map_err(|_| McpError::ConnectionClosed)?;
            Ok(())
        } else {
            Err(McpError::NotConnected)
        }
    }

    pub async fn close(&mut self) -> Result<(), McpError> {
        if let Some(cmd_tx) = &self.cmd_tx {
            let _ = cmd_tx.send(TransportCommand::Close).await;
        }
        self.cmd_tx = None;
        self.event_rx = None;
        Ok(())
    }

    pub fn set_request_handler(
        &mut self,
        method: &str,
        handler: RequestHandler,
    ) {
        self.assert_request_handler_capability(method)
            .expect("Invalid request handler capability");
            
        self.request_handlers
            .blocking_write()
            .insert(method.to_string(), handler);
    }

    pub fn set_notification_handler(
        &mut self,
        method: &str,
        handler: NotificationHandler,
    ) {
        self.notification_handlers
            .blocking_write()
            .insert(method.to_string(), handler);
    }

    fn register_default_handlers(&mut self) {
        // Handle cancellation notifications
        self.set_notification_handler(
            "cancelled",
            Box::new(|notification| {
                Box::pin(async move {
                    let params = notification.params
                        .ok_or(McpError::InvalidParams)?;
                    
                    let cancelled: CancelledNotification = serde_json::from_value(params)
                        .map_err(|_| McpError::InvalidParams)?;

                    // The cancelled notification contains:
                    // - request_id: the ID of the request being cancelled
                    // - reason: why the request was cancelled
                    tracing::debug!(
                        "Request {} cancelled: {}",
                        cancelled.request_id,
                        cancelled.reason
                    );
                    
                    Ok(())
                })
            })
        );

        // Handle progress notifications
        let progress_handlers = Arc::clone(&self.progress_handlers);
        self.set_notification_handler(
            "progress",
            Box::new(move |notification| {
                let progress_handlers = Arc::clone(&progress_handlers);
                Box::pin(async move {
                    let params = notification.params
                        .ok_or(McpError::InvalidParams)?;
                    
                    let progress: ProgressNotification = serde_json::from_value(params)
                        .map_err(|_| McpError::InvalidParams)?;

                    // Find and call the progress handler for this token
                    if let Some(handler) = progress_handlers.read().await.get(&progress.progress_token) {
                        handler(Progress {
                            progress: progress.progress,
                            total: progress.total,
                        });
                    }
                    
                    Ok(())
                })
            })
        );

        // Handle ping requests
        self.set_request_handler(
            "ping",
            Box::new(|_request, _extra| {
                Box::pin(async {
                    // Respond with an empty object as per protocol
                    Ok(serde_json::json!({}))
                })
            })
        );

        // Handle initialize request 
        self.set_request_handler(
            "initialize",
            Box::new(|request, _extra| {
                Box::pin(async move {
                    // Parse initialization parameters
                    let params = request.params
                        .ok_or(McpError::InvalidParams)?;
                    
                    let init_params: InitializeParams = serde_json::from_value(params)
                        .map_err(|_| McpError::InvalidParams)?;

                    // Respond with server capabilities
                    Ok(serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "resources": {
                                "subscribe": true,
                                "listChanged": true
                            }
                            // Add other capabilities as needed
                        }
                    }))
                })
            })
        );

        // Handle shutdown request
        self.set_request_handler(
            "shutdown",
            Box::new(|_request, _extra| {
                Box::pin(async {
                    // Prepare for shutdown (cleanup could go here)
                    Ok(serde_json::json!(null))
                })
            })
        );
    }




    // Protected methods that should be implemented by subclasses
    fn assert_capability_for_method(&self, method: &str) -> Result<(), McpError> {
        // Subclasses should implement this
        Ok(())
    }

    fn assert_notification_capability(&self, method: &str) -> Result<(), McpError> {
        // Subclasses should implement this
        Ok(())
    }

    fn assert_request_handler_capability(&self, method: &str) -> Result<(), McpError> {
        // Subclasses should implement this
        Ok(())
    }


}

// Helper types for JSON-RPC
#[derive(Debug, Serialize, Deserialize)]
pub struct CancelledNotification {
    pub request_id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProgressNotification {
    pub progress: u64,
    pub total: Option<u64>,
    pub progress_token: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct InitializeParams {
    #[serde(default = "default_protocol_version")]
    protocol_version: String,
    capabilities: Capabilities,
}

#[derive(Debug, Deserialize)]
struct Capabilities {
    #[serde(default)]
    resources: Option<ResourceCapabilities>,
    // Add other capability types as needed
}

fn default_protocol_version() -> String {
    "2024-11-05".to_string()
}

#[derive(Debug, Serialize)]
struct InitializeResult {
    protocol_version: String,
    capabilities: ServerCapabilities,
}

#[derive(Debug, Serialize)]
struct ServerCapabilities {
    resources: ResourceCapabilities,
    // Add other capability types as needed
}