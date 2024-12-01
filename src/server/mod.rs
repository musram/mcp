use std::{sync::Arc, time::Duration};

use config::ServerConfig;
use tracing::info;

use crate::{error::McpError, protocol::{Protocol, ProtocolBuilder, ProtocolOptions}, resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager}, tools::{CallToolRequest, ListToolsRequest}, transport::{SseTransport, StdioTransport}};
use crate::tools::{ToolManager, ToolCapabilities};

pub mod config;

pub struct McpServer {
    pub config: ServerConfig,
    pub resource_manager: Arc<ResourceManager>,
    pub tool_manager: Arc<ToolManager>,
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
        
        let resource_manager = Arc::new(ResourceManager::new(resource_capabilities));
        let tool_manager = Arc::new(ToolManager::new(tool_capabilities));
        
        Self {
            config,
            resource_manager,
            tool_manager,
        }
    }

    async fn run_stdio_transport(&self) -> Result<(), McpError> {
        let transport = StdioTransport::new(self.config.server.max_connections);
        let protocol = Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }));

        // Register resource handlers and build protocol
        let mut protocol = self.register_resource_handlers(protocol).build();
        
        // Connect transport
        protocol.connect(transport).await?;
        
        // Keep the server running
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn run_sse_transport(&self) -> Result<(), McpError> {
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
        
        // Keep the server running
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
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
        builder.with_request_handler(
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
        )
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
