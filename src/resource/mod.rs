use async_trait::async_trait;
use mime_guess::MimeGuess;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, path::PathBuf};
use tokio::sync::RwLock;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::error::McpError;

// Resource Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTemplate {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

// Request/Response types
#[derive(Debug, Deserialize)]
pub struct ListResourcesRequest {
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListResourcesResponse {
    pub resources: Vec<Resource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReadResourceRequest {
    pub uri: String,
}

#[derive(Debug, Serialize)]
pub struct ReadResourceResponse {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Serialize)]
pub struct ListTemplatesResponse {
    pub resource_templates: Vec<ResourceTemplate>,
}

// Resource Provider trait
#[async_trait]
pub trait ResourceProvider: Send + Sync {
    /// List available resources
    async fn list_resources(&self, cursor: Option<String>) -> Result<(Vec<Resource>, Option<String>), McpError>;
    
    /// Read resource contents
    async fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, McpError>;
    
    /// List available templates
    async fn list_templates(&self) -> Result<Vec<ResourceTemplate>, McpError>;
    
    /// Check if URI is supported
    async fn supports_uri(&self, uri: &str) -> bool;
    
    /// Validate URI format and access permissions
    async fn validate_uri(&self, uri: &str) -> Result<(), McpError>;
}

// Resource Manager
pub struct ResourceManager {
    pub providers: Arc<RwLock<HashMap<String, Arc<dyn ResourceProvider>>>>,
    pub subscriptions: Arc<RwLock<HashMap<String, Vec<String>>>>,
    pub capabilities: ResourceCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCapabilities {
    pub subscribe: bool,
    pub list_changed: bool,
}

impl ResourceManager {
    pub fn new(capabilities: ResourceCapabilities) -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            capabilities,
        }
    }

    pub async fn register_provider(&self, scheme: String, provider: Arc<dyn ResourceProvider>) {
        let mut providers = self.providers.write().await;
        providers.insert(scheme, provider);
    }

    pub async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResponse, McpError> {
        let providers = self.providers.read().await;
        let mut all_resources = Vec::new();
        let mut next_cursor = None;

        for provider in providers.values() {
            let (mut resources, provider_cursor) = provider.list_resources(cursor.clone()).await?;
            all_resources.append(&mut resources);
            if provider_cursor.is_some() {
                next_cursor = provider_cursor;
            }
        }

        Ok(ListResourcesResponse {
            resources: all_resources,
            next_cursor,
        })
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResponse, McpError> {
        let providers = self.providers.read().await;
        
        // Extract scheme from URI
        let scheme = uri.split("://").next()
            .ok_or_else(|| McpError::InvalidRequest("Invalid URI format".to_string()))?;
            
        let provider = providers.get(scheme)
            .ok_or_else(|| McpError::ResourceNotFound(uri.to_string()))?;

        // Validate URI before reading
        provider.validate_uri(uri).await?;
        
        let contents = provider.read_resource(uri).await?;
        Ok(ReadResourceResponse { contents })
    }

    pub async fn list_templates(&self) -> Result<ListTemplatesResponse, McpError> {
        let providers = self.providers.read().await;
        let mut all_templates = Vec::new();

        for provider in providers.values() {
            let mut templates = provider.list_templates().await?;
            all_templates.append(&mut templates);
        }

        Ok(ListTemplatesResponse {
            resource_templates: all_templates,
        })
    }

    pub async fn subscribe(&self, client_id: String, uri: String) -> Result<(), McpError> {
        if !self.capabilities.subscribe {
            return Err(McpError::CapabilityNotSupported("subscribe".to_string()));
        }

        let mut subscriptions = self.subscriptions.write().await;
        subscriptions
            .entry(uri)
            .or_insert_with(Vec::new)
            .push(client_id);
        Ok(())
    }

    pub async fn unsubscribe(&self, client_id: &str, uri: &str) -> Result<(), McpError> {
        if !self.capabilities.subscribe {
            return Err(McpError::CapabilityNotSupported("subscribe".to_string()));
        }

        let mut subscriptions = self.subscriptions.write().await;
        if let Some(subscribers) = subscriptions.get_mut(uri) {
            subscribers.retain(|id| id != client_id);
            if subscribers.is_empty() {
                subscriptions.remove(uri);
            }
        }
        Ok(())
    }

    pub async fn notify_resource_updated(&self, _uri: &str) -> Result<(), McpError> {
        if !self.capabilities.subscribe {
            return Err(McpError::CapabilityNotSupported("subscribe".to_string()));
        }

        // Notification handling would go here
        // This would typically integrate with your protocol implementation
        Ok(())
    }

    pub async fn notify_list_changed(&self) -> Result<(), McpError> {
        if !self.capabilities.list_changed {
            return Err(McpError::CapabilityNotSupported("listChanged".to_string()));
        }

        // List changed notification handling would go here
        // This would typically integrate with your protocol implementation
        Ok(())
    }
}

// File System Resource Provider Implementation
pub struct FileSystemProvider {
    root_path: PathBuf,

}

impl FileSystemProvider {
    pub fn new<P: Into<PathBuf>>(root_path: P) -> Self {
        Self {
            root_path: root_path.into(),
          
        }
    }

    fn sanitize_path(&self, uri: &str) -> Result<PathBuf, McpError> {
        let path = uri.strip_prefix("file://")
            .ok_or_else(|| McpError::InvalidRequest("Invalid file URI".to_string()))?;
            
        let full_path = self.root_path.join(path);
        if !full_path.starts_with(&self.root_path) {
            return Err(McpError::AccessDenied("Path traversal attempt detected".to_string()));
        }
        
        Ok(full_path)
    }

    fn is_text_content(&self, mime_type: &str, content: &[u8]) -> bool {
        // Check if it's a known text format
        if mime_type.starts_with("text/") ||
           mime_type == "application/json" ||
           mime_type == "application/javascript" ||
           mime_type == "application/xml" {
            return true;
        }

        // Fallback: Check if content looks like UTF-8 text
        String::from_utf8(content.to_vec()).is_ok()
    }
}

#[async_trait]
impl ResourceProvider for FileSystemProvider {
    async fn list_resources(&self, _cursor: Option<String>) -> Result<(Vec<Resource>, Option<String>), McpError> {
        let mut resources = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.root_path).await.map_err(|_e| McpError::IoError)?;
        
        while let Some(entry) = entries.next_entry().await.map_err(|_e| McpError::IoError)? {
            let path = entry.path();
          
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let mime_type = MimeGuess::from_path(path.clone()).first().as_ref().map(|s| s.to_string());
                   
                
                resources.push(Resource {
                    uri: format!("file://{}", path.to_string_lossy()),
                    name: name.to_string(),
                    description: None,
                    mime_type
                });
            }
        }

        Ok((resources, None))
    }

    async fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, McpError> {
        let path = self.sanitize_path(uri)?;
        
        if !path.exists() {
            return Err(McpError::ResourceNotFound(uri.to_string()));
        }

        let content = tokio::fs::read(&path).await.map_err(|_e| McpError::IoError)?;
        let mime_type = MimeGuess::from_path(&path)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let resource_content = if self.is_text_content(&mime_type, &content) {
            let text = String::from_utf8(content)
                .map_err(|_| McpError::InvalidResource("Invalid UTF-8".to_string()))?;
            ResourceContent {
                uri: uri.to_string(),
                mime_type: Some(mime_type),
                text: Some(text),
                blob: None,
            }
        } else {
            ResourceContent {
                uri: uri.to_string(),
                mime_type: Some(mime_type),
                text: None,
                blob: Some(BASE64.encode(&content)),
            }
        };

        Ok(vec![resource_content])
    }

    async fn list_templates(&self) -> Result<Vec<ResourceTemplate>, McpError> {
        Ok(vec![ResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "Project Files".to_string(),
            description: Some("Access files in the project directory".to_string()),
            mime_type: None,
        }])
    }

    async fn supports_uri(&self, uri: &str) -> bool {
        uri.starts_with("file://") && self.sanitize_path(uri).is_ok()
    }

    async fn validate_uri(&self, uri: &str) -> Result<(), McpError> {
        self.sanitize_path(uri)?;
        Ok(())
    }
}

// Add test module
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_json_resource_loading() -> Result<(), McpError> {
        // Create temp directory
        let temp_dir = TempDir::new().unwrap();
        let provider = FileSystemProvider::new(temp_dir.path());

        // Create test JSON file
        let json_path = temp_dir.path().join("test.json");
        fs::write(&json_path, r#"{"test": "value"}"#).unwrap();

        // Test JSON file reading
        let uri = format!("file://{}", json_path.to_string_lossy());
        let contents = provider.read_resource(&uri).await?;
        
        assert_eq!(contents.len(), 1);
        let content = &contents[0];
        assert!(content.text.is_some());
        assert!(content.blob.is_none());
        assert_eq!(content.mime_type.as_deref(), Some("application/json"));

        Ok(())
    }

    
}