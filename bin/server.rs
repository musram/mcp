use clap::Parser;
use mcp_rs::logging::McpSubscriber;
use mcp_rs::{
    error::McpError,
    prompts::Prompt,
    resource::FileSystemProvider,
    server::{
        config::{ResourceSettings, ServerConfig, ServerSettings, TransportType},
        McpServer,
    },
    tools::calculator::CalculatorTool,
};
use std::{path::PathBuf, sync::Arc};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
    #[arg(short, long)]
    transport: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), McpError> {
    // Parse command line arguments
    let args = Args::parse();

    // Load or create config
    let config = if let Some(config_path) = args.config {
        // Load from file
        let config_str = std::fs::read_to_string(config_path)?;
        let mut server_config: ServerConfig = serde_json::from_str(&config_str)?;
        if let Some(transport) = args.transport {
            server_config.server.transport = TransportType::from(transport.as_str());
        }
        server_config
    } else {
        // Create default config with CLI overrides
        let workspace = args.workspace.unwrap_or_else(|| PathBuf::from("."));

        ServerConfig {
            server: ServerSettings {
                name: "mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                transport: TransportType::from(args.transport.as_deref().unwrap_or("stdio")),
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

    let transport = config.server.transport.clone();

    // Log startup info
    tracing::info!(
        "Starting MCP server v{} with {} transport",
        config.server.version,
        match config.server.transport {
            TransportType::Stdio => "STDIO",
            TransportType::Sse => "SSE",
            TransportType::WebSocket => "WebSocket",
        }
    );

    let resources_root_path = config.resources.root_path.clone();
    let logging_level = config.logging.level.clone();

    // Create server instance
    let mut server = McpServer::new(config).await;

    // Set up logging with both standard and MCP subscribers
    let mcp_subscriber = McpSubscriber::new(Arc::clone(&server.logging_manager));

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_line_number(true),
        )
        .with(mcp_subscriber)
        .init();

    // Set initial log level from config
    server
        .logging_manager
        .lock()
        .await
        .set_level(logging_level.clone())
        .await?;

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
    tracing::info!("  Logging: enabled (level: {})", logging_level);
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

    // Start server based on transport type
    match transport {
        TransportType::Stdio => {
            tracing::info!("Starting server with STDIO transport");

            // Run server and wait for shutdown
            tokio::select! {
                result = server.run_stdio_transport() => {
                    if let Err(e) = result {
                        tracing::error!("Server error: {}", e);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Shutting down server...");
                }
            }
        }
        TransportType::Sse => {
            tracing::info!("Starting server with SSE transport");

            // Run server and wait for shutdown
            tokio::select! {
                result = server.run_sse_transport() => {
                    if let Err(e) = result {
                        tracing::error!("Server error: {}", e);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Shutting down server...");
                }
            }
        }
        TransportType::WebSocket => {
            unimplemented!("WebSocket transport not implemented");
        }
    }

    Ok(())
}
