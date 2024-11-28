use async_trait::async_trait;

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use mcp_typegen::mcp_types::{JsonrpcMessage as JsonRpcMessage, JsonrpcRequest as JsonRpcRequest, JsonrpcResponse as JsonRpcResponse, JsonrpcNotification as JsonRpcNotification};
use mcp_typegen::{McpError, Request, Notification, Result as McpResult, JsonSchema};


/// Default request timeout in milliseconds
pub const DEFAULT_REQUEST_TIMEOUT_MSEC: u64 = 60000;
/// Callback for progress notifications
pub type ProgressCallback = Box<dyn Fn(Progress) + Send + Sync>;
/// Protocol options
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

/// Request options
#[derive(Debug, Clone)]
pub struct RequestOptions {
    /// Progress callback
    pub on_progress: Option<ProgressCallback>,
    /// Cancellation signal
    pub cancel_signal: Option<CancellationToken>,
    /// Request timeout in milliseconds
    pub timeout: Option<u64>,
}
/// Extra data for request handlers
#[derive(Debug, Clone)]
pub struct RequestHandlerExtra {
    /// Cancellation signal
    pub signal: CancellationToken,
}
/// Transport trait for communication
#[async_trait]
pub trait Transport: Send + Sync {
    async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>>;
    async fn send(&self, message: JsonRpcMessage) -> Result<(), Box<dyn std::error::Error>>;
    async fn close(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}
/// Protocol implementation
pub struct Protocol<SR, SN, ST>
where
    SR: Request + Send + Sync,
    SN: Notification + Send + Sync,
    ST: McpResult + Send + Sync,
{
    transport: Option<Box<dyn Transport>>,
    request_message_id: Arc<Mutex<i64>>,
    request_handlers: Arc<Mutex<HashMap<String, RequestHandler<ST>>>>,
    notification_handlers: Arc<Mutex<HashMap<String, NotificationHandler>>>,
    response_handlers: Arc<Mutex<HashMap<i64, ResponseHandler>>>,
    progress_handlers: Arc<Mutex<HashMap<i64, ProgressCallback>>>,
    options: ProtocolOptions,
    on_close: Option<Box<dyn Fn() + Send + Sync>>,
    on_error: Option<Box<dyn Fn(Box<dyn std::error::Error>) + Send + Sync>>,
}
type RequestHandler<T> = Box<
    dyn Fn(
            JsonRpcRequest,
            RequestHandlerExtra,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<T, McpError>> + Send>>
        + Send
        + Sync,
>;
type NotificationHandler = Box<
    dyn Fn(
            JsonRpcNotification,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), McpError>> + Send>>
        + Send
        + Sync,
>;
type ResponseHandler = oneshot::Sender<Result<JsonRpcResponse, McpError>>;

impl<SR, SN, ST> Protocol<SR, SN, ST>
where
    SR: Request + Send + Sync + 'static,
    SN: Notification + Send + Sync + 'static,
    ST: McpResult + Send + Sync + 'static,
{
    pub fn new(options: Option<ProtocolOptions>) -> Self {
        let options = options.unwrap_or_default();
        let mut protocol = Self {
            transport: None,
            request_message_id: Arc::new(Mutex::new(0)),
            request_handlers: Arc::new(Mutex::new(HashMap::new())),
            notification_handlers: Arc::new(Mutex::new(HashMap::new())),
            response_handlers: Arc::new(Mutex::new(HashMap::new())),
            progress_handlers: Arc::new(Mutex::new(HashMap::new())),
            options,
            on_close: None,
            on_error: None,
        };

        // Set up default handlers
        protocol.set_notification_handler(
            "notifications/cancelled",
            Box::new(|notification| {
                Box::pin(async move {
                    // Handle cancellation
                    Ok(())
                })
            }),
        );

        protocol.set_notification_handler(
            "notifications/progress",
            Box::new(|notification| {
                Box::pin(async move {
                    // Handle progress
                    Ok(())
                })
            }),
        );

        protocol.set_request_handler(
            "ping",
            Box::new(|_, _| Box::pin(async move { Ok(ST::default()) })),
        );

        protocol
    }

    pub async fn connect(
        &mut self,
        transport: Box<dyn Transport>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.transport = Some(transport);
        if let Some(transport) = &mut self.transport {
            transport.start().await?;
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(transport) = &mut self.transport {
            transport.close().await?;
        }
        if let Some(on_close) = &self.on_close {
            on_close();
        }
        Ok(())
    }

    pub async fn request<T>(
        &self,
        request: SR,
        result_schema: T,
        options: Option<RequestOptions>,
    ) -> Result<T::Output, McpError>
    where
        T: JsonSchema,
    {
        // Implementation of request method
        todo!()
    }

    pub async fn notification(&self, notification: SN) -> Result<(), McpError> {
        // Implementation of notification method
        todo!()
    }
}
