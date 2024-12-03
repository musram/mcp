use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;
use std::path::PathBuf;

use crate::{
    error::McpError,
    tools::{Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult},
};

pub struct SearchTool;

impl SearchTool {
    pub fn new() -> Self {
        Self
    }

    async fn search_directory(dir: PathBuf, pattern: &str, results: &mut Vec<String>) -> Result<(), McpError> {
        // Box the recursive future
        Box::pin(Self::search_directory_recursive(dir, pattern, results)).await
    }

    #[async_recursion::async_recursion]
    async fn search_directory_recursive(dir: PathBuf, pattern: &str, results: &mut Vec<String>) -> Result<(), McpError> {
        let mut entries = fs::read_dir(&dir).await.map_err(|_| McpError::IoError)?;
        
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let file_name = path.file_name()
                .and_then(|n| n.to_str())
                .ok_or(McpError::IoError)?
                .to_lowercase();

            if file_name.contains(&pattern.to_lowercase()) {
                results.push(path.to_string_lossy().to_string());
            }

            if path.is_dir() {
                Self::search_directory_recursive(path, pattern, results).await?;
            }
        }
        
        Ok(())
    }

    async fn get_file_info(path: &str) -> Result<String, McpError> {
        let metadata = fs::metadata(path).await.map_err(|_| McpError::IoError)?;
        
        let file_type = if metadata.is_dir() { "Directory" } else { "File" };
        let size = metadata.len();
        let modified = metadata.modified()
            .map_err(|_| McpError::IoError)?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Ok(format!(
            "Type: {}\nSize: {} bytes\nLast Modified: {} seconds since epoch",
            file_type, size, modified
        ))
    }
}

#[async_trait]
impl ToolProvider for SearchTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["search_files", "get_file_info"]
            }),
        );
        schema_properties.insert(
            "path".to_string(),
            json!({
                "type": "string"
            }),
        );
        schema_properties.insert(
            "pattern".to_string(),
            json!({
                "type": "string"
            }),
        );

        Tool {
            name: "search".to_string(),
            description: "Search for files and get file information.".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: schema_properties,
                required: vec!["operation".to_string()],
            },
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError> {
        match arguments["operation"].as_str() {
            Some("search_files") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                let pattern = arguments["pattern"].as_str().ok_or(McpError::InvalidParams)?;
                
                let mut results = Vec::new();
                Self::search_directory(PathBuf::from(path), pattern, &mut results).await?;
                
                Ok(ToolResult {
                    content: vec![ToolContent::Text { 
                        text: if results.is_empty() {
                            "No files found".to_string()
                        } else {
                            results.join("\n")
                        }
                    }],
                    is_error: false,
                })
            }
            Some("get_file_info") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                let info = Self::get_file_info(path).await?;
                
                Ok(ToolResult {
                    content: vec![ToolContent::Text { text: info }],
                    is_error: false,
                })
            }
            _ => Err(McpError::InvalidParams),
        }
    }
}
