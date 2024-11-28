use std::{sync::Arc, time::Duration};

use config::ServerConfig;
use tracing::info;

use crate::{error::McpError, protocol::{Protocol, ProtocolOptions}, resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager}, transport::{SseTransport, StdioTransport}};

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

    pub async fn run_stdio_transport(&self) -> Result<(), McpError> {
        info!("Starting stdio transport");
        
        let transport = StdioTransport::new(self.config.server.max_connections);
        let mut protocol = Protocol::new(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }));

        // Register resource handlers
        self.register_resource_handlers(&mut protocol).await?;
        
        // Connect transport
        protocol.connect(transport).await?;
        
        // Keep the server running
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn run_sse_transport(&self) -> Result<(), McpError> {
        info!("Starting SSE transport on {}:{}", self.config.server.host, self.config.server.port);
        
        let transport = SseTransport::new(
            self.config.server.port,
            self.config.server.max_connections,
        );
        let mut protocol = Protocol::new(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }));

        // Register resource handlers
        self.register_resource_handlers(&mut protocol).await?;
        
        // Connect transport
        protocol.connect(transport).await?;
        
        // Keep the server running
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn register_resource_handlers(&self, protocol: &mut Protocol) -> Result<(), McpError> {
        let resource_manager = self.resource_manager.clone();

        // List resources handler
        protocol.set_request_handler(
            "resources/list",
            Box::new(move |request, _extra| {
                let rm = resource_manager.clone();
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
            }),
        );
        let resource_manager = self.resource_manager.clone();
        // Read resource handler
        protocol.set_request_handler(
            "resources/read",
            Box::new(move |request, _extra| {
                let rm = resource_manager.clone();
                Box::pin(async move {
                    let params: ReadResourceRequest = serde_json::from_value(request.params.unwrap()).unwrap();
                    rm.read_resource(&params.uri).await
                        .map(|response| serde_json::to_value(response).unwrap())
                        .map_err(|e| e.into())
                })
            }),
        );
        let resource_manager = self.resource_manager.clone();
        // List templates handler
        protocol.set_request_handler(
            "resources/templates/list",
            Box::new(move |_request, _extra| {
                let rm = resource_manager.clone();
                Box::pin(async move {
                    rm.list_templates().await
                        .map(|response| serde_json::to_value(response).unwrap())
                        .map_err(|e| e.into())
                })
            }),
        );
        let resource_manager = self.resource_manager.clone();
        // Subscribe handler
        if self.resource_manager.capabilities.subscribe {
            protocol.set_request_handler(
                "resources/subscribe",
                Box::new(move |request, _extra| {
                    let rm = resource_manager.clone();
                    Box::pin(async move {
                        let params = serde_json::from_value(request.params.unwrap()).unwrap();
                        rm.subscribe(request.id.to_string(), params).await
                            .map(|_| serde_json::json!({}))
                    })
                }),
            );
        }

        Ok(())
    }

    pub async fn run(&mut self)  -> Result<(), McpError> {
        match &self.config.server.transport {
            config::TransportType::Stdio => {
                self.run_stdio_transport().await
            },
            config::TransportType::Sse =>  {
                self.run_sse_transport().await
            }
            config::TransportType::WebSocket => {
                unimplemented!()
            }

        }
    }
}