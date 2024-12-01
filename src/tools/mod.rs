use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

pub mod calculator;

use crate::error::McpError;

// Tool Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource {
        resource: ResourceContent,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}

// Request/Response types
#[derive(Debug, Deserialize)]
pub struct ListToolsRequest {
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListToolsResponse {
    pub tools: Vec<Tool>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallToolRequest {
    pub name: String,
    pub arguments: Value,
}

// Tool Provider trait
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Get tool definition
    async fn get_tool(&self) -> Tool;
    
    /// Execute tool
    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError>;
}

// Tool Manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilities {
    pub list_changed: bool,
}

pub struct ToolManager {
    pub tools: Arc<RwLock<HashMap<String, Arc<dyn ToolProvider>>>>,
    pub capabilities: ToolCapabilities,
}

impl ToolManager {
    pub fn new(capabilities: ToolCapabilities) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            capabilities,
        }
    }

    pub async fn register_tool(&self, provider: Arc<dyn ToolProvider>) {
        let tool = provider.get_tool().await;
        let mut tools = self.tools.write().await;
        tools.insert(tool.name, provider);
    }

    pub async fn list_tools(&self, _cursor: Option<String>) -> Result<ListToolsResponse, McpError> {
        let tools = self.tools.read().await;
        let mut tool_list = Vec::new();
        
        for provider in tools.values() {
            tool_list.push(provider.get_tool().await);
        }

        Ok(ListToolsResponse {
            tools: tool_list,
            next_cursor: None, // Implement pagination if needed
        })
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, McpError> {
        let tools = self.tools.read().await;
        let provider = tools.get(name)
            .ok_or_else(|| McpError::InvalidRequest(format!("Unknown tool: {}", name)))?;
            
        provider.execute(arguments).await
    }
}
