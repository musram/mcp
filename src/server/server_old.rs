pub mod runner;
pub mod server;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use mcp_typegen::mcp_types::{
        ClientCapabilities, Implementation, InitializeRequest, InitializeRequestParams,
    };
    use serde_json::Map;
    use server::{ListPromptsHandler, NotificationOptions, Server};
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn test_server() {
        let server = Server::builder("my_server".to_string())
            .with_request_handler("list_prompts", ListPromptsHandler {})
            .with_notification_options(NotificationOptions::default())
            .build();

        // Set up channels and run server
        let (tx, rx) = mpsc::channel(100);

        let init_req = InitializeRequest {
            method: "initialize".to_string(),
            params: InitializeRequestParams {
                client_info: Implementation {
                    name: "my_server".to_string(),
                    version: "0.1".to_string(),
                },
                capabilities: ClientCapabilities {
                    roots: None,
                    sampling: Map::new(),
                    experimental: HashMap::new(),
                },
                protocol_version: "2.0".to_string(),
            },
        };

        server.run(rx, tx, init_req).await;
    }
}
