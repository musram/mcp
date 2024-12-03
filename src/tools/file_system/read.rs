use std::collections::HashMap;
use async_trait::async_trait;
use futures::future::{try_join_all, Future};
use serde_json::{json, Value};
use tokio::fs;

use crate::{
    error::McpError,
    tools::{Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult},
};

pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }

    async fn read_single_file(path: &str) -> Result<String, McpError> {
        fs::read_to_string(path)
            .await
            .map_err(|e| {
                tracing::error!("Failed to read file {}: {}", path, e);
                McpError::IoError
            })
    }

    async fn read_multiple_files(paths: &[String]) -> Result<Vec<(String, Result<String, McpError>)>, McpError> {
        let futures: Vec<_> = paths.iter().map(|path| {
            let path = path.clone();
            async move {
                let result = Self::read_single_file(&path).await;
                Ok((path, result))
            }
        }).collect();

        try_join_all(futures).await
    }
}

#[async_trait]
impl ToolProvider for ReadFileTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["read_file", "read_multiple_files"]
            }),
        );
        schema_properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Path to the file to read"
            }),
        );
        schema_properties.insert(
            "paths".to_string(),
            json!({
                "type": "array",
                "items": {
                    "type": "string"
                },
                "description": "List of file paths to read"
            }),
        );

        Tool {
            name: "read_file".to_string(),
            description: "Read the complete contents of one or more files from the file system. \
                Handles various text encodings and provides detailed error messages.".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: schema_properties,
                required: vec!["operation".to_string()],
            },
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError> {
        match arguments["operation"].as_str() {
            Some("read_file") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                let content = Self::read_single_file(path).await?;
                
                Ok(ToolResult {
                    content: vec![ToolContent::Text { text: content }],
                    is_error: false,
                })
            }
            Some("read_multiple_files") => {
                let paths = arguments["paths"]
                    .as_array()
                    .ok_or(McpError::InvalidParams)?
                    .iter()
                    .filter_map(|p| p.as_str().map(String::from))
                    .collect::<Vec<_>>();

                let results = Self::read_multiple_files(&paths).await?;
                let mut contents = Vec::new();

                for (path, result) in results {
                    match result {
                        Ok(content) => contents.push(ToolContent::Text { 
                            text: format!("File: {}\n{}", path, content) 
                        }),
                        Err(e) => contents.push(ToolContent::Text { 
                            text: format!("Error reading {}: {}", path, e) 
                        }),
                    }
                }

                Ok(ToolResult {
                    content: contents,
                    is_error: false,
                })
            }
            _ => Err(McpError::InvalidParams),
        }
    }
}
