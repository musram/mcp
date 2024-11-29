use clap::Parser;
use config::{Config, Environment, File};
use mcp::error::McpError;
use std::path::PathBuf;

use mcp::server::McpServer;
use mcp::server::config::{ServerConfig, TransportType};

// CLI Arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Config file path
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Transport type (stdio, sse, websocket)
    #[arg(short, long, default_value = "sse")]
    transport: String,

    /// Server port for network transports
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Log level (error, warn, info, debug, trace)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Resource root directory
    #[arg(short, long)]
    resource_root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), McpError> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    
    // Load configuration
    let mut config = Config::builder();
    
    // Add default config
    config = config.add_source(Config::try_from(&ServerConfig::default()).unwrap());
    
    // Add config file if specified
    if let Some(config_path) = cli.config {
        config = config.add_source(File::from(config_path));
    }
    
    // Add environment variables
    config = config.add_source(Environment::with_prefix("MCP"));
    
    // Build config
    let mut server_config: ServerConfig = config.build().unwrap().try_deserialize().unwrap();
    
    // Override with CLI arguments
    if let Some(resource_root) = cli.resource_root {
        server_config.resources.root_path = resource_root;
    }
    
    server_config.logging.level = cli.log_level;
    server_config.server.port = cli.port;
    server_config.server.transport = match cli.transport.as_str() {
        "stdio" => TransportType::Stdio,
        "sse" => TransportType::Sse,
        "websocket" => TransportType::WebSocket,
        _ => return Err(McpError::Custom {
            code: -32000,
            message: "Invalid transport type".to_string(),
        }),
    };
    
    // Create and run server
    let mut server = McpServer::new(server_config);
    server.run().await
}