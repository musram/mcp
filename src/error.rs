use std::fmt;

// Core error types
#[derive(Debug)]
pub enum McpError {
    ParseError,
    InvalidRequest(String),
    SerializationError,
    MethodNotFound,
    InvalidParams,
    InternalError,
    NotConnected,
    ConnectionClosed,
    RequestTimeout,
    ResourceNotFound(String),
    InvalidResource(String),
    AccessDenied(String),
    IoError,
    CapabilityNotSupported(String),
    Custom { code: i32, message: String },
}

impl McpError {
    pub fn code(&self) -> i32 {
        match self {
            McpError::ParseError => -32700,
            McpError::InvalidRequest(_) => -32600,
            McpError::SerializationError => -32603,
            McpError::MethodNotFound => -32601,
            McpError::InvalidParams => -32602,
            McpError::InternalError => -32603,
            McpError::NotConnected => -32000,
            McpError::ConnectionClosed => -32001,
            McpError::RequestTimeout => -32002,
            McpError::ResourceNotFound(_) => -32003,
            McpError::InvalidResource(_) => -32004,
            McpError::IoError => -32005,
            McpError::CapabilityNotSupported(_) => -32006,
            McpError::AccessDenied(_) => -32007,
            McpError::Custom { code, .. } => *code,
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            McpError::ParseError => write!(f, "Parse error"),
            McpError::InvalidRequest(s) => write!(f, "Invalid request: {}", s),
            McpError::MethodNotFound => write!(f, "Method not found"),
            McpError::InvalidParams => write!(f, "Invalid parameters"),
            McpError::InternalError => write!(f, "Internal error"),
            McpError::NotConnected => write!(f, "Not connected"),
            McpError::ConnectionClosed => write!(f, "NConnection closed"),
            McpError::RequestTimeout => write!(f, "Request timeout"),
            McpError::IoError => write!(f, "io error"),
            McpError::SerializationError => write!(f, "Serialization error"),
            McpError::ResourceNotFound(s) => write!(f, " {} Resource not found", s),
            McpError::InvalidResource(s) => write!(f, "{} Invalid resource", s),
            McpError::AccessDenied(s) => write!(f, "Access denied: {}", s),
            McpError::CapabilityNotSupported(s) => write!(f, "Capability not supported: {}", s),
            McpError::Custom { code, message } => write!(f, "Error {}: {}", code, message),
        }
    }
}

impl std::error::Error for McpError {}

impl From<serde_json::Error> for McpError {
    fn from(_error: serde_json::Error) -> Self {
        McpError::SerializationError
    }
}

impl From<std::io::Error> for McpError {
    fn from(_error: std::io::Error) -> Self {
        McpError::IoError
    }
}

impl From<tokio::time::error::Elapsed> for McpError {
    fn from(_error: tokio::time::error::Elapsed) -> Self {
        McpError::RequestTimeout
    }
}
