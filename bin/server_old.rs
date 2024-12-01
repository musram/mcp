use clap::Parser;
use mcp_rs::{
    error::McpError,
    prompts::Prompt,
    resource::FileSystemProvider,
    server::{
        config::{
            LoggingSettings, ResourceSettings, ServerConfig, ServerSettings, ToolSettings,
            TransportType,
        },
        McpServer,
    },
    tools::calculator::CalculatorTool,
};
use std::{path::PathBuf, sync::Arc};
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Path to workspace directory
    #[arg(short, long)]
    workspace: Option<PathBuf>,

    /// Server port
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Transport type (stdio, sse, ws)
    #[arg(short, long, default_value = "stdio")]
    transport: String,
}

#[tokio::main]
async fn main() -> Result<(), McpError> {
    // Parse command line arguments
    let args = Args::parse();

    // Set up logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_span_events(FmtSpan::CLOSE)
        .init();

    // Load or create config
    let config = if let Some(config_path) = args.config {
        // Load from file
        let config_str = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&config_str)?
    } else {
        // Create default config with CLI overrides
        let workspace = args.workspace.unwrap_or_else(|| PathBuf::from("."));

        ServerConfig {
            server: ServerSettings {
                name: "mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                transport: match args.transport.as_str() {
                    "stdio" => TransportType::Stdio,
                    "sse" => TransportType::Sse,
                    "ws" => TransportType::WebSocket,
                    _ => TransportType::Stdio,
                },
                host: "127.0.0.1".to_string(),
                port: args.port,
                max_connections: 100,
                timeout_ms: 30000,
            },
            resources: ResourceSettings {
                root_path: workspace,
                allowed_schemes: vec!["file".to_string()],
                max_file_size: 10 * 1024 * 1024,
                enable_templates: true,
            },
            ..ServerConfig::default()
        }
    };

    // Log startup info
    tracing::info!(
        "Starting MCP server v{} with {} transport on port {}",
        config.server.version,
        match config.server.transport {
            TransportType::Stdio => "STDIO",
            TransportType::Sse => "SSE",
            TransportType::WebSocket => "WebSocket",
        },
        config.server.port
    );

    let resources_root_path = config.resources.root_path.clone();

    // Create server instance
    let mut server = McpServer::new(config);

    // Register file system provider
    let fs_provider = Arc::new(FileSystemProvider::new(&resources_root_path));
    server
        .resource_manager
        .register_provider("file".to_string(), fs_provider)
        .await;

    // Register calculator tool
    let calculator = Arc::new(CalculatorTool::new());
    server.tool_manager.register_tool(calculator).await;

    // Register some example prompts
    let code_review_prompt = Prompt {
        name: "code_review".to_string(),
        description: "Review code for quality and suggest improvements".to_string(),
        arguments: vec![
            mcp_rs::prompts::PromptArgument {
                name: "code".to_string(),
                description: "The code to review".to_string(),
                required: true,
            },
            mcp_rs::prompts::PromptArgument {
                name: "language".to_string(),
                description: "Programming language".to_string(),
                required: false,
            },
        ],
    };
    server
        .prompt_manager
        .register_prompt(code_review_prompt)
        .await;

    let explain_code_prompt = Prompt {
        name: "explain_code".to_string(),
        description: "Explain how code works in plain language".to_string(),
        arguments: vec![mcp_rs::prompts::PromptArgument {
            name: "code".to_string(),
            description: "The code to explain".to_string(),
            required: true,
        }],
    };
    server
        .prompt_manager
        .register_prompt(explain_code_prompt)
        .await;

    // List capabilities
    tracing::info!("Enabled capabilities:");
    tracing::info!("  Resources:");
    tracing::info!(
        "    - subscribe: {}",
        server.resource_manager.capabilities.subscribe
    );
    tracing::info!(
        "    - listChanged: {}",
        server.resource_manager.capabilities.list_changed
    );
    tracing::info!("  Tools:");
    tracing::info!(
        "    - listChanged: {}",
        server.tool_manager.capabilities.list_changed
    );
    tracing::info!("  Prompts:");
    tracing::info!(
        "    - listChanged: {}",
        server.prompt_manager.capabilities.list_changed
    );

    // Run server
    server.run().await?;

    Ok(())
}
