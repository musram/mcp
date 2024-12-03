use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use crate::{
    error::McpError,
    tools::{Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult},
};

pub struct DirectoryTool;

impl DirectoryTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolProvider for DirectoryTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["create_directory", "list_directory", "move_file"]
            }),
        );
        schema_properties.insert(
            "path".to_string(),
            json!({
                "type": "string"
            }),
        );
        schema_properties.insert(
            "source".to_string(),
            json!({
                "type": "string"
            }),
        );
        schema_properties.insert(
            "destination".to_string(),
            json!({
                "type": "string"
            }),
        );

        Tool {
            name: "directory".to_string(),
            description: "Directory operations including create, list, and move files.".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: schema_properties,
                required: vec!["operation".to_string()],
            },
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError> {
        match arguments["operation"].as_str() {
            Some("create_directory") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                fs::create_dir_all(path).await.map_err(|_| McpError::IoError)?;
                
                Ok(ToolResult {
                    content: vec![ToolContent::Text { 
                        text: format!("Created directory: {}", path) 
                    }],
                    is_error: false,
                })
            }
            Some("list_directory") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                let mut entries = fs::read_dir(path).await.map_err(|_| McpError::IoError)?;
                let mut listing = Vec::new();

                while let Ok(Some(entry)) = entries.next_entry().await {
                    let file_type = entry.file_type().await.map_err(|_| McpError::IoError)?;
                    let prefix = if file_type.is_dir() { "[DIR]" } else { "[FILE]" };
                    listing.push(format!("{} {}", prefix, entry.file_name().to_string_lossy()));
                }

                Ok(ToolResult {
                    content: vec![ToolContent::Text { 
                        text: listing.join("\n") 
                    }],
                    is_error: false,
                })
            }
            Some("move_file") => {
                let source = arguments["source"].as_str().ok_or(McpError::InvalidParams)?;
                let destination = arguments["destination"].as_str().ok_or(McpError::InvalidParams)?;
                
                fs::rename(source, destination).await.map_err(|_| McpError::IoError)?;
                
                Ok(ToolResult {
                    content: vec![ToolContent::Text { 
                        text: format!("Moved {} to {}", source, destination) 
                    }],
                    is_error: false,
                })
            }
            _ => Err(McpError::InvalidParams),
        }
    }
}
