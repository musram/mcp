use std::{collections::HashMap, sync::Arc};
use async_trait::async_trait;
use serde_json::json;
use tokio;

use mcp_rs::{
    error::McpError, server::{config::ServerConfig, McpServer}, tools::{Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult}
};

// Mock tool provider for testing
struct MockCalculatorTool;

#[async_trait]
impl ToolProvider for MockCalculatorTool {
    async fn get_tool(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "description": "Operation to perform (add, subtract, multiply, divide)"
            }),
        );
        properties.insert(
            "a".to_string(),
            json!({
                "type": "number",
                "description": "First operand"
            }),
        );
        properties.insert(
            "b".to_string(),
            json!({
                "type": "number",
                "description": "Second operand"
            }),
        );
        Tool {
            name: "calculator".to_string(),
            description: "Performs basic arithmetic operations".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties,
                required: vec!["operation", "a", "b"].iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolResult, McpError> {
        let params: CalculatorParams = serde_json::from_value(arguments)
            .map_err(|_| McpError::InvalidParams)?;

        let result = match params.operation.as_str() {
            "add" => params.a + params.b,
            "subtract" => params.a - params.b,
            "multiply" => params.a * params.b,
            "divide" => {
                if params.b == 0.0 {
                    return Ok(ToolResult {
                        content: vec![ToolContent::Text { 
                            text: "Division by zero".to_string() 
                        }],
                        is_error: true,
                    });
                }
                params.a / params.b
            },
            _ => return Err(McpError::InvalidParams),
        };

        Ok(ToolResult {
            content: vec![ToolContent::Text { 
                text: result.to_string() 
            }],
            is_error: false,
        })
    }
}

#[derive(serde::Deserialize)]
struct CalculatorParams {
    operation: String,
    a: f64,
    b: f64,
}

#[tokio::test]
async fn test_tool_registration_and_listing() {
    // Create test server
    let config = ServerConfig::default();
    let server = McpServer::new(config).await;
    
    // Register mock tool
    let tool_provider = Arc::new(MockCalculatorTool);
    server.tool_manager.register_tool(tool_provider).await;

    // Test listing tools
    let response = server.tool_manager.list_tools(None).await.unwrap();
    assert_eq!(response.tools.len(), 1);
    assert_eq!(response.tools[0].name, "calculator");
}

#[tokio::test]
async fn test_tool_execution() {
    // Create test server
    let config = ServerConfig::default();
    let server = McpServer::new(config).await;
    
    // Register mock tool
    let tool_provider = Arc::new(MockCalculatorTool);
    server.tool_manager.register_tool(tool_provider).await;

    // Test addition
    let result = server.tool_manager.call_tool(
        "calculator",
        json!({
            "operation": "add",
            "a": 5,
            "b": 3
        })
    ).await.unwrap();

    match &result.content[0] {
        ToolContent::Text { text } => assert_eq!(text, "8"),
        _ => panic!("Expected text content"),
    }
    assert!(!result.is_error);

    // Test division by zero
    let result = server.tool_manager.call_tool(
        "calculator",
        json!({
            "operation": "divide",
            "a": 1,
            "b": 0
        })
    ).await.unwrap();

    match &result.content[0] {
        ToolContent::Text { text } => assert_eq!(text, "Division by zero"),
        _ => panic!("Expected text content"),
    }
    assert!(result.is_error);
}

#[tokio::test]
async fn test_invalid_tool() {
    // Create test server
    let config = ServerConfig::default();
    let server = McpServer::new(config).await;

    // Test calling non-existent tool
    let result = server.tool_manager.call_tool(
        "nonexistent",
        json!({})
    ).await;

    assert!(result.is_err());
    match result {
        Err(McpError::InvalidRequest(msg)) => {
            assert!(msg.contains("Unknown tool"));
        },
        _ => panic!("Expected InvalidRequest error"),
    }
}

#[tokio::test]
async fn test_invalid_arguments() {
    // Create test server
    let config = ServerConfig::default();
    let server = McpServer::new(config).await;
    
    // Register mock tool
    let tool_provider = Arc::new(MockCalculatorTool);
    server.tool_manager.register_tool(tool_provider).await;

    // Test invalid operation
    let result = server.tool_manager.call_tool(
        "calculator",
        json!({
            "operation": "invalid",
            "a": 1,
            "b": 2
        })
    ).await.unwrap();

    assert!(result.is_error);
}
