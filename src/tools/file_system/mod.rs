mod read;
mod write;
mod directory;
mod search;

use std::sync::Arc;
use std::path::PathBuf;
use async_trait::async_trait;
use serde_json::Value;
use crate::{error::McpError, tools::{Tool, ToolProvider, ToolResult, ToolContent}};

#[derive(Clone)]
pub struct FileSystemTools {
    read_tool: Arc<read::ReadFileTool>,
    write_tool: Arc<write::WriteFileTool>,
    directory_tool: Arc<directory::DirectoryTool>,
    search_tool: Arc<search::SearchTool>,
    allowed_directories: Arc<Vec<PathBuf>>,
}

impl FileSystemTools {
    pub fn new() -> Self {
        Self {
            read_tool: Arc::new(read::ReadFileTool::new()),
            write_tool: Arc::new(write::WriteFileTool::new()),
            directory_tool: Arc::new(directory::DirectoryTool::new()),
            search_tool: Arc::new(search::SearchTool::new()),
            allowed_directories: Arc::new(vec![std::env::current_dir().unwrap()]),
        }
    }

    pub fn with_allowed_directories(allowed_dirs: Vec<PathBuf>) -> Self {
        Self {
            read_tool: Arc::new(read::ReadFileTool::new()),
            write_tool: Arc::new(write::WriteFileTool::new()),
            directory_tool: Arc::new(directory::DirectoryTool::new()),
            search_tool: Arc::new(search::SearchTool::new()),
            allowed_directories: Arc::new(allowed_dirs),
        }
    }

    pub async fn validate_path(&self, requested_path: &str) -> Result<PathBuf, McpError> {
        let requested_path = PathBuf::from(requested_path);
        let absolute = if requested_path.is_absolute() {
            requested_path.clone()
        } else {
            std::env::current_dir().unwrap().join(requested_path.clone())
        };

        let normalized = absolute.canonicalize()
            .map_err(|e| {
                tracing::error!("Path validation error for {}: {}", requested_path.display(), e);
                McpError::IoError
            })?;
        
        for allowed_dir in self.allowed_directories.iter() {
            if (normalized.starts_with(allowed_dir)) {
                return Ok(normalized);
            }
        }

        Err(McpError::InvalidParams)
    }
}

#[async_trait]
impl ToolProvider for FileSystemTools {
    async fn get_tool(&self) -> Tool {
        // Return composite tool definition containing all file system operations
        let mut tools = vec![
            self.read_tool.get_tool().await,
            self.write_tool.get_tool().await,
            self.directory_tool.get_tool().await,
            self.search_tool.get_tool().await,
        ];
        
        // Return the first tool as the main tool definition
        tools.remove(0)
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError> {
        // Add operation to list allowed directories
        if arguments["operation"].as_str() == Some("list_allowed_directories") {
            let dirs = self.allowed_directories.iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            
            return Ok(ToolResult {
                content: vec![ToolContent::Text { text: dirs }],
                is_error: false,
            });
        }

        // Route to appropriate sub-tool based on operation type
        let operation = arguments["operation"].as_str().ok_or(McpError::InvalidParams)?;
        
        match operation {
            "read_file" | "read_multiple_files" => self.read_tool.execute(arguments).await,
            "write_file" => self.write_tool.execute(arguments).await,
            "create_directory" | "list_directory" | "move_file" => self.directory_tool.execute(arguments).await,
            "search_files" | "get_file_info" => self.search_tool.execute(arguments).await,
            _ => Err(McpError::InvalidParams),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use serde_json::json;

    async fn setup_test_env() -> (FileSystemTools, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let allowed_dirs = vec![temp_dir.path().to_path_buf()];
        let fs_tools = FileSystemTools::with_allowed_directories(allowed_dirs);
        (fs_tools, temp_dir)
    }

    #[tokio::test]
    async fn test_file_operations() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        let test_file = temp_dir.path().join("test.txt");
        let test_content = "Hello, world!";

        // Test write operation
        let write_result = fs_tools.execute(json!({
            "operation": "write_file",
            "path": test_file.to_str().unwrap(),
            "content": test_content,
        })).await.unwrap();
        assert!(!write_result.is_error);

        // Test read operation
        let read_result = fs_tools.execute(json!({
            "operation": "read_file",
            "path": test_file.to_str().unwrap(),
        })).await.unwrap();
        
        match &read_result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, test_content),
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_directory_operations() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        let test_dir = temp_dir.path().join("test_dir");

        // Test directory creation
        let create_result = fs_tools.execute(json!({
            "operation": "create_directory",
            "path": test_dir.to_str().unwrap(),
        })).await.unwrap();
        assert!(!create_result.is_error);

        // Test directory listing
        let list_result = fs_tools.execute(json!({
            "operation": "list_directory",
            "path": temp_dir.path().to_str().unwrap(),
        })).await.unwrap();

        match &list_result.content[0] {
            ToolContent::Text { text } => assert!(text.contains("[DIR] test_dir")),
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_search_operations() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        
        // Create test files
        let test_files = vec!["test1.txt", "test2.txt", "other.txt"];
        for file in &test_files {
            let path = temp_dir.path().join(file);
            fs_tools.execute(json!({
                "operation": "write_file",
                "path": path.to_str().unwrap(),
                "content": "test content",
            })).await.unwrap();
        }

        // Test search
        let search_result = fs_tools.execute(json!({
            "operation": "search_files",
            "path": temp_dir.path().to_str().unwrap(),
            "pattern": "test",
        })).await.unwrap();

        match &search_result.content[0] {
            ToolContent::Text { text } => {
                assert!(text.contains("test1.txt"));
                assert!(text.contains("test2.txt"));
                assert!(!text.contains("other.txt"));
            },
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_move_operations() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        let source = temp_dir.path().join("source.txt");
        let dest = temp_dir.path().join("dest.txt");

        // Create source file
        fs_tools.execute(json!({
            "operation": "write_file",
            "path": source.to_str().unwrap(),
            "content": "test content",
        })).await.unwrap();

        // Test move operation
        let move_result = fs_tools.execute(json!({
            "operation": "move_file",
            "source": source.to_str().unwrap(),
            "destination": dest.to_str().unwrap(),
        })).await.unwrap();
        assert!(!move_result.is_error);

        // Verify source doesn't exist and destination does
        assert!(!source.exists());
        assert!(dest.exists());
    }

    #[tokio::test]
    async fn test_path_validation() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        let invalid_path = "/tmp/invalid/path";

        // Test invalid path
        let result = fs_tools.execute(json!({
            "operation": "write_file",
            "path": invalid_path,
            "content": "test content",
        })).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_file_operations() {
        let (fs_tools, temp_dir) = setup_test_env().await;
        
        // Create test files
        let files = vec!["multi1.txt", "multi2.txt"];
        for (i, file) in files.iter().enumerate() {
            let path = temp_dir.path().join(file);
            fs_tools.execute(json!({
                "operation": "write_file",
                "path": path.to_str().unwrap(),
                "content": format!("content {}", i),
            })).await.unwrap();
        }

     

        // Test reading multiple files
        let read_result = fs_tools.execute(json!({
            "operation": "read_multiple_files",
            "paths": files.iter().map(|f| temp_dir.path().join(f).to_str().unwrap().to_string()).collect::<Vec<_>>(),
        })).await.unwrap();

        assert_eq!(read_result.content.len(), 2);
        match &read_result.content[0] {
            ToolContent::Text { text } => assert!(text.contains("content 0")),
            _ => panic!("Expected text content"),
        }
    }
}
