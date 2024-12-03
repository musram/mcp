use async_trait::async_trait;

use crate::{error::McpError, server};

use super::{Tool, ToolContent, ToolProvider, ToolResult};

pub struct TestTool;

impl TestTool {
    pub fn new() -> Self {
        TestTool
    }
}

#[async_trait]
impl ToolProvider for TestTool {
    async fn get_tool(&self) -> Tool {
        Tool {
            name: "test_tool".to_string(),
            description: "Test Tool".to_string(),
            input_schema: serde_json::from_str(r#"{
                "type": "object",
                "properties": {
                    "test": {
                        "type": "string",
                        "description": "Test property"
                    }
                },
                "required": ["test"]
            }"#).unwrap(),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolResult, McpError> {
        Ok(ToolResult {
            content: vec![],
            is_error: false,
        })
    }
}


pub struct PingTool;

impl PingTool {
    pub fn new() -> Self {
        PingTool
    }
}

#[async_trait]
impl ToolProvider for PingTool {
    async fn get_tool(&self) -> Tool {
        Tool {
            name: "ping_tool".to_string(),
            description: "Ping Tool".to_string(),
            input_schema: serde_json::from_str(r#"{
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "Server to ping"
                    }
                },
                "required": ["server"]
            }"#).unwrap(),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolResult, McpError> {
        let server = arguments.get("server").map(|s| s.as_str()).flatten().unwrap_or("localhost");
        let res = reqwest::get(format!("http://{}", server)).await.map_err(|e| McpError::ToolExecutionError(e.to_string()))?;
        let body = res.text().await.map_err(|e| McpError::ToolExecutionError(e.to_string()))?;
        Ok(ToolResult {
            content: vec![ToolContent::Text { text: body }],
            is_error: false,
        })
    }
}