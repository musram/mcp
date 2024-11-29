use std::{sync::Arc, time::Duration};

use config::ServerConfig;
use tracing::info;

use crate::{error::McpError, protocol::{Protocol, ProtocolOptions, ProtocolBuilder}, resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager}, transport::{SseTransport, StdioTransport}};

pub mod config;

pub struct McpServer {
    config: ServerConfig,
    resource_manager: Arc<ResourceManager>,
}

impl McpServer {
    pub fn new(config: ServerConfig) -> Self {
        let capabilities = ResourceCapabilities {
            subscribe: true,
            list_changed: true,
        };
        let resource_manager = Arc::new(ResourceManager::new(capabilities));
        Self {
            config,
            resource_manager,
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
        // Add this line to store a cloned reference
        let resource_manager = Arc::clone(&self.resource_manager);

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
        
        if self.resource_manager.capabilities.subscribe {
            let resource_manager = Arc::clone(&self.resource_manager);
            let builder = builder.with_request_handler(
                "resources/subscribe",
                Box::new(move |request, _extra| {
                    let rm = Arc::clone(&resource_manager);
                    Box::pin(async move {
                        let params = serde_json::from_value(request.params.unwrap()).unwrap();
                        rm.subscribe(request.id.to_string(), params).await
                            .map(|_| serde_json::json!({}))
                    })
                }),
            );
            builder
        } else {
            builder
        }
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