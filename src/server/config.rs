use std::path::PathBuf;

use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

// Server Configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub resources: ResourceSettings,
    pub security: SecuritySettings,
    pub logging: LoggingSettings,
    pub tools: ToolSettings,  // Add this
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerSettings {
    pub name: String,
    pub version: String,
    pub transport: TransportType,
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceSettings {
    pub root_path: PathBuf,
    pub allowed_schemes: Vec<String>,
    pub max_file_size: usize,
    pub enable_templates: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub enable_auth: bool,
    pub token_secret: Option<String>,
    pub rate_limit: RateLimitSettings,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RateLimitSettings {
    pub requests_per_minute: u32,
    pub burst_size: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoggingSettings {
    pub level: String,
    pub file: Option<PathBuf>,
    pub format: LogFormat,
}

// Add new tool settings struct
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolSettings {
    pub enabled: bool,
    pub require_confirmation: bool,
    pub allowed_tools: Vec<String>,
    pub max_execution_time_ms: u64,
    pub rate_limit: RateLimitSettings,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Stdio,
    Sse,
    WebSocket,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Pretty,
    Compact,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            server: ServerSettings {
                name: "mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                transport: TransportType::Stdio,
                host: "127.0.0.1".to_string(),
                port: 3000,
                max_connections: 100,
                timeout_ms: 30000,
            },
            resources: ResourceSettings {
                root_path: PathBuf::from("./resources"),
                allowed_schemes: vec!["file".to_string()],
                max_file_size: 10 * 1024 * 1024, // 10MB
                enable_templates: true,
            },
            security: SecuritySettings {
                enable_auth: false,
                token_secret: None,
                rate_limit: RateLimitSettings {
                    requests_per_minute: 60,
                    burst_size: 10,
                },
                allowed_origins: vec!["*".to_string()],
            },
            logging: LoggingSettings {
                level: "info".to_string(),
                file: None,
                format: LogFormat::Pretty,
            },
            tools: ToolSettings {
                enabled: true,
                require_confirmation: true,
                allowed_tools: vec!["*".to_string()], // Allow all tools by default
                max_execution_time_ms: 30000, // 30 seconds
                rate_limit: RateLimitSettings {
                    requests_per_minute: 30,
                    burst_size: 5,
                },
            },
        }
    }
}
