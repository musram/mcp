use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use crate::{error::McpError, protocol::JsonRpcNotification, NotificationSender};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub arguments: Vec<PromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessageContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource {
        uri: String,
        mime_type: String,
        text: Option<String>,
        blob: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    pub description: String,
    pub messages: Vec<PromptMessage>,
}

// Request/Response types
#[derive(Debug, Deserialize, Serialize)]
pub struct ListPromptsRequest {
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListPromptsResponse {
    pub prompts: Vec<Prompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetPromptRequest {
    pub name: String,
    pub arguments: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCapabilities {
    pub list_changed: bool,
}

pub struct PromptManager {
    pub prompts: Arc<RwLock<HashMap<String, Prompt>>>,
    pub capabilities: PromptCapabilities,
    pub notification_sender: Option<NotificationSender>,
}

impl PromptManager {
    pub fn new(capabilities: PromptCapabilities) -> Self {
        Self {
            prompts: Arc::new(RwLock::new(HashMap::new())),
            capabilities,
            notification_sender: None,
        }
    }

    pub fn set_notification_sender(&mut self, sender: NotificationSender) {
        self.notification_sender = Some(sender);
    }

    pub async fn register_prompt(&self, prompt: Prompt) {
        let mut prompts = self.prompts.write().await;
        prompts.insert(prompt.name.clone(), prompt);
        
        // Notify about list change
        self.notify_list_changed().await.ok();
    }

    pub async fn list_prompts(&self, _cursor: Option<String>) -> Result<ListPromptsResponse, McpError> {
        let prompts = self.prompts.read().await;
        let prompts: Vec<_> = prompts.values().cloned().collect();
        
        Ok(ListPromptsResponse {
            prompts,
            next_cursor: None, // Implement pagination if needed
        })
    }

    pub async fn get_prompt(&self, name: &str, arguments: Option<serde_json::Value>) -> Result<PromptResult, McpError> {
        let prompts = self.prompts.read().await;
        let prompt = prompts.get(name)
            .ok_or_else(|| McpError::InvalidRequest(format!("Unknown prompt: {}", name)))?;

        // Validate required arguments
        if let Some(args) = &arguments {
            for arg in prompt.arguments.iter().filter(|a| a.required) {
                if !args.get(&arg.name).is_some() {
                    return Err(McpError::InvalidRequest(
                        format!("Missing required argument: {}", arg.name)
                    ));
                }
            }
        } else if prompt.arguments.iter().any(|a| a.required) {
            return Err(McpError::InvalidRequest("Missing required arguments".to_string()));
        }

        // Here you would generate the actual prompt messages based on the template
        // This is a simple example
        Ok(PromptResult {
            description: prompt.description.clone(),
            messages: vec![
                PromptMessage {
                    role: "user".to_string(),
                    content: MessageContent::Text { 
                        text: format!("Using prompt: {}", prompt.name)
                    },
                },
            ],
        })
    }

    async fn notify_list_changed(&self) -> Result<(), McpError> {
        if !self.capabilities.list_changed {
            return Ok(());
        }

        if let Some(sender) = &self.notification_sender {
            let notification = JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "notifications/prompts/list_changed".to_string(),
                params: None,
            };

            sender.tx.send(notification).await
                .map_err(|e| McpError::InternalError(format!("Notification error: {}", e)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prompt_registration() {
        let manager = PromptManager::new(PromptCapabilities {
            list_changed: false,
        });

        let prompt = Prompt {
            name: "test".to_string(),
            description: "Test prompt".to_string(),
            arguments: vec![
                PromptArgument {
                    name: "arg1".to_string(),
                    description: "First argument".to_string(),
                    required: true,
                },
            ],
        };

        manager.register_prompt(prompt.clone()).await;

        let result = manager.list_prompts(None).await.unwrap();
        assert_eq!(result.prompts.len(), 1);
        assert_eq!(result.prompts[0].name, "test");
    }

    #[tokio::test]
    async fn test_get_prompt() {
        let manager = PromptManager::new(PromptCapabilities {
            list_changed: false,
        });

        let prompt = Prompt {
            name: "test".to_string(),
            description: "Test prompt".to_string(),
            arguments: vec![],
        };

        manager.register_prompt(prompt).await;

        let result = manager.get_prompt("test", None).await.unwrap();
        assert_eq!(result.messages.len(), 1);
    }
}
