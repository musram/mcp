use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::{
    error::McpError, logging::LoggingCapabilities, prompts::{GetPromptRequest, ListPromptsRequest, ListPromptsResponse, PromptCapabilities, PromptResult}, protocol::{JsonRpcNotification, Protocol, ProtocolOptions}, resource::{ListResourcesRequest, ListResourcesResponse, ReadResourceRequest, ReadResourceResponse, ResourceCapabilities}, tools::{CallToolRequest, ListToolsRequest, ListToolsResponse, ToolCapabilities, ToolResult}, transport::Transport
};

// Client capabilities and info structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapabilities {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

// Server response structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub logging: Option<LoggingCapabilities>,
    pub prompts: Option<PromptCapabilities>,
    pub resources: Option<ResourceCapabilities>,
    pub tools: Option<ToolCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}


pub struct Client {
    protocol: Protocol,
    initialized: Arc<RwLock<bool>>,
    server_capabilities: Arc<RwLock<Option<ServerCapabilities>>>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            protocol: Protocol::builder(Some(ProtocolOptions {
                enforce_strict_capabilities: true,
            })).build(),
            initialized: Arc::new(RwLock::new(false)),
            server_capabilities: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn connect<T: Transport>(&mut self, transport: T) -> Result<(), McpError> {
        self.protocol.connect(transport).await
    }

    pub async fn initialize(&mut self, client_info: ClientInfo) -> Result<InitializeResult, McpError> {
        // Ensure we're not already initialized
        if *self.initialized.read().await {
            return Err(McpError::InvalidRequest("Client already initialized".to_string()));
        }

        // Prepare initialization parameters
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {
                roots: Some(RootsCapabilities {
                    list_changed: true,
                }),
                sampling: Some(SamplingCapabilities {}),
            },
            client_info,
        };

        // Send initialize request
        let result: InitializeResult = self.protocol.request(
            "initialize",
            Some(params),
            None,
        ).await?;

        // Validate protocol version
        if result.protocol_version != "2024-11-05" {
            return Err(McpError::InvalidRequest(format!(
                "Unsupported protocol version: {}",
                result.protocol_version
            )));
        }

        // Store server capabilities
        *self.server_capabilities.write().await = Some(result.capabilities.clone());

        // Send initialized notification
        self.protocol.notification(
            "initialized",
            Option::<()>::None,
        ).await?;

        // Mark as initialized
        *self.initialized.write().await = true;

        Ok(result)
    }

    // Resource methods
    pub async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol.request(
            "resources/list",
            Some(ListResourcesRequest { cursor }),
            None,
        ).await
    }

    pub async fn read_resource(&self, uri: String) -> Result<ReadResourceResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol.request(
            "resources/read",
            Some(ReadResourceRequest { uri }),
            None,
        ).await
    }

    pub async fn subscribe_to_resource(&self, uri: String) -> Result<(), McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol.request(
            "resources/subscribe",
            Some(uri),
            None,
        ).await
    }

    // Prompt methods
    pub async fn list_prompts(&self, cursor: Option<String>) -> Result<ListPromptsResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("prompts").await?;

        self.protocol.request(
            "prompts/list",
            Some(ListPromptsRequest { cursor }),
            None,
        ).await
    }

    pub async fn get_prompt(&self, name: String, arguments: Option<serde_json::Value>) -> Result<PromptResult, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("prompts").await?;

        self.protocol.request(
            "prompts/get",
            Some(GetPromptRequest { name, arguments }),
            None,
        ).await
    }

    // Tool methods
    pub async fn list_tools(&self, cursor: Option<String>) -> Result<ListToolsResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("tools").await?;

        self.protocol.request(
            "tools/list",
            Some(ListToolsRequest { cursor }),
            None,
        ).await
    }

    pub async fn call_tool(&self, name: String, arguments: serde_json::Value) -> Result<ToolResult, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("tools").await?;

        self.protocol.request(
            "tools/call",
            Some(CallToolRequest { name, arguments }),
            None,
        ).await
    }

    // Logging methods
    pub async fn set_log_level(&self, level: String) -> Result<(), McpError> {
        self.assert_initialized().await?;
        self.assert_capability("logging").await?;

        self.protocol.request(
            "logging/setLevel",
            Some(serde_json::json!({ "level": level })),
            None,
        ).await
    }

    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        if !*self.initialized.read().await {
            return Err(McpError::InvalidRequest("Client not initialized".to_string()));
        }

        self.protocol.close().await
    }

    pub async fn assert_initialized(&self) -> Result<(), McpError> {
        if !*self.initialized.read().await {
            return Err(McpError::InvalidRequest("Client not initialized".to_string()));
        }
        Ok(())
    }

    async fn assert_capability(&self, capability: &str) -> Result<(), McpError> {
        let caps = self.server_capabilities.read().await;
        let caps = caps.as_ref().ok_or_else(|| McpError::InvalidRequest("No server capabilities".to_string()))?;

        let has_capability = match capability {
            "logging" => caps.logging.is_some(),
            "prompts" => caps.prompts.is_some(),
            "resources" => caps.resources.is_some(),
            "tools" => caps.tools.is_some(),
            _ => false,
        };

        if !has_capability {
            return Err(McpError::CapabilityNotSupported(capability.to_string()));
        }

        Ok(())
    }

    pub async fn get_server_capabilities(&self) -> Option<ServerCapabilities> {
        self.server_capabilities.read().await.clone()
    }

    // Helper method to check if server supports a capability
    pub async fn has_capability(&self, capability: &str) -> bool {
        if let Some(caps) = self.get_server_capabilities().await {
            match capability {
                "logging" => caps.logging.is_some(),
                "prompts" => caps.prompts.is_some(),
                "resources" => caps.resources.is_some(),
                "tools" => caps.tools.is_some(),
                _ => false,
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::StdioTransport;

    #[tokio::test]
    async fn test_client_lifecycle() -> Result<(), McpError> {
        let mut client = Client::new();
        
        // Connect using stdio transport
        let transport = StdioTransport::new(32);
        client.connect(transport).await?;

        // Initialize client
        let result = client.initialize(ClientInfo {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        }).await?;

        // Test some requests
        let resources = client.list_resources(None).await?;
        assert!(!resources.resources.is_empty());

        let prompts = client.list_prompts(None).await?;
        assert!(!prompts.prompts.is_empty());

        // Shutdown
        client.shutdown().await?;

        Ok(())
    }
}
