use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use crate::{
    error::McpError,
    tools::{Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult},
};

pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolProvider for WriteFileTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["write_file"]
            }),
        );
        schema_properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Path to the file to write to"
            }),
        );
        schema_properties.insert(
            "content".to_string(),
            json!({
                "type": "string",
                "description": "Content to write to the file"
            }),
        );

        Tool {
            name: "write_file".to_string(),
            description: "Write content to a file. Creates a new file or overwrites existing one.".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: schema_properties,
                required: vec!["operation".to_string(), "path".to_string(), "content".to_string()],
            },
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError> {
        let path = arguments["path"]
            .as_str()
            .ok_or(McpError::InvalidParams)?;
        let content = arguments["content"]
            .as_str()
            .ok_or(McpError::InvalidParams)?;

        // Write the file
        fs::write(path, content)
            .await
            .map_err(|_| McpError::IoError)?;

        Ok(ToolResult {
            content: vec![ToolContent::Text { 
                text: format!("Successfully wrote {} bytes to {}", content.len(), path) 
            }],
            is_error: false,
        })
    }
}
