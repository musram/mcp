use std::{sync::{Arc, Mutex}, time::Duration};

use config::ServerConfig;
use tracing::info;

use crate::{error::McpError, protocol::{JsonRpcNotification, Protocol, ProtocolBuilder, ProtocolOptions}, resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager}, tools::{CallToolRequest, ListToolsRequest}, transport::{SseTransport, StdioTransport}, NotificationSender};
use crate::tools::{ToolManager, ToolCapabilities};
use crate::prompts::{GetPromptRequest, ListPromptsRequest, PromptCapabilities, PromptManager};
use tokio::sync::mpsc;
use crate::logging::{LoggingManager, SetLevelRequest};

pub mod config;

pub struct McpServer {
    pub config: ServerConfig,
    pub resource_manager: Arc<ResourceManager>,
    pub tool_manager: Arc<ToolManager>,
    pub prompt_manager: Arc<PromptManager>,
    pub logging_manager: Arc<tokio::sync::Mutex<LoggingManager>>,
    notification_tx: mpsc::Sender<JsonRpcNotification>,
    notification_rx: Option<mpsc::Receiver<JsonRpcNotification>>, // Make this Option
}

impl McpServer {
    pub fn new(config: ServerConfig) -> Self {
        let resource_capabilities = ResourceCapabilities {
            subscribe: true,
            list_changed: true,
        };
        let tool_capabilities = ToolCapabilities {
            list_changed: true,
        };
        
        // Create channel for notifications with enough capacity
        let (notification_tx, notification_rx) = mpsc::channel(100);
        
        let mut resource_manager = Arc::new(ResourceManager::new(resource_capabilities));
        // Set up notification sender
        Arc::get_mut(&mut resource_manager)
            .unwrap()
            .set_notification_sender(NotificationSender { 
                tx: notification_tx.clone() 
            });
            
        let tool_manager = Arc::new(ToolManager::new(tool_capabilities));
        
        let prompt_capabilities = PromptCapabilities {
            list_changed: true,
        };
        
        let mut prompt_manager = Arc::new(PromptManager::new(prompt_capabilities));
        Arc::get_mut(&mut prompt_manager)
            .unwrap()
            .set_notification_sender(NotificationSender { 
                tx: notification_tx.clone() 
            });
        
        let mut logging_manager = LoggingManager::new();
        logging_manager.set_notification_sender(NotificationSender { 
            tx: notification_tx.clone() 
        });
        let logging_manager = Arc::new(tokio::sync::Mutex::new(logging_manager));

        Self {
            config,
            resource_manager,
            tool_manager,
            prompt_manager,
            logging_manager,
            notification_tx,
            notification_rx: Some(notification_rx), // Wrap in Some
        }
    }

    async fn handle_notifications(
        mut notification_rx: mpsc::Receiver<JsonRpcNotification>,
        protocol: Arc<Protocol>
    ) {
        while let Some(notification) = notification_rx.recv().await {
            if let Err(e) = protocol.send_notification(notification).await {
                tracing::error!("Failed to send notification: {:?}", e);
            }
        }
    }

    async fn run_stdio_transport(&mut self) -> Result<(), McpError> {
        let transport = StdioTransport::new(self.config.server.max_connections);
        let protocol = Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }));

        // Register resource handlers and build protocol
        let mut protocol = self.register_resource_handlers(protocol).build();
        
        // Connect transport
        protocol.connect(transport).await?;
        
        // Create notification handler
        let protocol = Arc::new(protocol);
        let notification_handler = {
            let protocol = Arc::clone(&protocol);
            // Take ownership of the receiver
            let notification_rx = self.notification_rx.take()
                .ok_or_else(|| McpError::InternalError)?;
            tokio::spawn(Self::handle_notifications(notification_rx, protocol))
        };
        
        // Keep the server running
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received shutdown signal");
            }
            _ = notification_handler => {
                tracing::error!("Notification handler terminated");
            }
        }

        Ok(())
    }

    async fn run_sse_transport(&mut self) -> Result<(), McpError> {
        let transport = SseTransport::new(
            self.config.server.port,
            self.config.server.max_connections,
        );
        let protocol = Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }));

        // Register resource handlers and build protocol
        let mut protocol = self.register_resource_handlers(protocol).build();
        
        // Connect transport
        protocol.connect(transport).await?;
        
        // Create notification handler
        let protocol = Arc::new(protocol);
        let notification_handler = {
            let protocol = Arc::clone(&protocol);
            // Take ownership of the receiver
            let notification_rx = self.notification_rx.take()
                .ok_or_else(|| McpError::InternalError)?;
            tokio::spawn(Self::handle_notifications(notification_rx, protocol))
        };

        // Keep the server running
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received shutdown signal");
            }
            _ = notification_handler => {
                tracing::error!("Notification handler terminated");
            }
        }

        Ok(())
    }

    fn register_resource_handlers(&self, builder: ProtocolBuilder) -> ProtocolBuilder {
        // Clone Arc references once at the beginning
        let resource_manager = Arc::clone(&self.resource_manager);
        let tool_manager = Arc::clone(&self.tool_manager);

        // Chain all handlers in a single builder flow
        let builder = builder
            .with_request_handler(
                "resources/list",
                Box::new(move |request, _extra| {
                    let rm = Arc::clone(&resource_manager);
                    Box::pin(async move {
                        let params: ListResourcesRequest = if let Some(params) = request.params {
                            serde_json::from_value(params).unwrap()
                        } else {
                            ListResourcesRequest { cursor: None }
                        };
                        
                        rm.list_resources(params.cursor).await
                            .map(|response| serde_json::to_value(response).unwrap())
                            .map_err(|e| e.into())
                    })
                })
            );

        // Clone for next handler
        let resource_manager = Arc::clone(&self.resource_manager);
        let builder = builder
            .with_request_handler(
                "resources/read",
                Box::new(move |request, _extra| {
                    let rm = Arc::clone(&resource_manager);
                    Box::pin(async move {
                        let params: ReadResourceRequest = serde_json::from_value(request.params.unwrap()).unwrap();
                        rm.read_resource(&params.uri).await
                            .map(|response| serde_json::to_value(response).unwrap())
                            .map_err(|e| e.into())
                    })
                })
            );

        // Clone for next handler
        let resource_manager = Arc::clone(&self.resource_manager);
        let builder = builder
            .with_request_handler(
                "resources/templates/list",
                Box::new(move |_request, _extra| {
                    let rm = Arc::clone(&resource_manager);
                    Box::pin(async move {
                        rm.list_templates().await
                            .map(|response| serde_json::to_value(response).unwrap())
                            .map_err(|e| e.into())
                    })
                })
            );

        // Clone for conditional handler
        let builder = if self.resource_manager.capabilities.subscribe {
            let resource_manager = Arc::clone(&self.resource_manager);
            builder.with_request_handler(
                "resources/subscribe",
                Box::new(move |request, _extra| {
                    let rm = Arc::clone(&resource_manager);
                    Box::pin(async move {
                        let params = serde_json::from_value(request.params.unwrap()).unwrap();
                        rm.subscribe(request.id.to_string(), params).await
                            .map(|_| serde_json::json!({}))
                    })
                })
            )
        } else {
            builder
        };

        // Add tool handlers
        let builder = builder
            .with_request_handler(
                "tools/list",
                Box::new(move |request, _extra| {
                    let tm = Arc::clone(&tool_manager);
                    Box::pin(async move {
                        let params: ListToolsRequest = if let Some(params) = request.params {
                            serde_json::from_value(params).map_err(|e| {
                                tracing::error!("Error parsing list tools request: {:?}", e);
                                
                                McpError::ParseError})?
                        } else {
                            ListToolsRequest { cursor: None }
                        };
                        
                        tm.list_tools(params.cursor).await
                            .map(|response| serde_json::to_value(response).unwrap())
                            
                            .map_err(|e| e.into())
                    })
                })
            );

        // Clone for final handler
        let tool_manager = Arc::clone(&self.tool_manager);
        let builder = builder.with_request_handler(
            "tools/call",
            Box::new(move |request, _extra| {
                let tm = Arc::clone(&tool_manager);
                Box::pin(async move {
                    let params: CallToolRequest = serde_json::from_value(request.params.unwrap()).unwrap();
                    tm.call_tool(&params.name, params.arguments).await
                        .map(|response| serde_json::to_value(response).unwrap())
                        .map_err(|e| e.into())
                })
            })
        );

        // Add prompt handlers
        let prompt_manager = Arc::clone(&self.prompt_manager);
        let builder = builder
            .with_request_handler(
                "prompts/list",
                Box::new(move |request, _extra| {
                    let pm = Arc::clone(&prompt_manager);
                    Box::pin(async move {
                        let params: ListPromptsRequest = if let Some(params) = request.params {
                            serde_json::from_value(params).unwrap()
                        } else {
                            ListPromptsRequest { cursor: None }
                        };
                        
                        pm.list_prompts(params.cursor).await
                            .map(|response| serde_json::to_value(response).unwrap())
                            .map_err(|e| e.into())
                    })
                })
            );

        let prompt_manager = Arc::clone(&self.prompt_manager);
        let builder = builder.with_request_handler(
            "prompts/get",
            Box::new(move |request, _extra| {
                let pm = Arc::clone(&prompt_manager);
                Box::pin(async move {
                    let params: GetPromptRequest = serde_json::from_value(request.params.unwrap()).unwrap();
                    pm.get_prompt(&params.name, params.arguments).await
                        .map(|response| serde_json::to_value(response).unwrap())
                        .map_err(|e| e.into())
                })
            })
        );

        // Add logging handlers
        let logging_manager = Arc::clone(&self.logging_manager);
        let builder = builder.with_request_handler(
            "logging/setLevel",
            Box::new(move |request, _extra| {
                let lm = Arc::clone(&logging_manager);
                Box::pin(async move {
                    let params: SetLevelRequest = serde_json::from_value(request.params.unwrap())?;
                    lm.lock().await.set_level(params.level).await?;
                    Ok(serde_json::json!({}))
                })
            })
        );

        builder
    }

    pub async fn run(&mut self) -> Result<(), McpError> {
        // Set up logging first if needed
        
        match &self.config.server.transport {
            config::TransportType::Stdio => {
                info!("Starting server with STDIO transport");
                self.run_stdio_transport().await
            },
            config::TransportType::Sse => {
                info!("Starting server with SSE transport");
                self.run_sse_transport().await
            },
            config::TransportType::WebSocket => {
                unimplemented!("WebSocket transport not implemented")
            }
        }
    }
}
