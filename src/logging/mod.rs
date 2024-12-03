use crate::{error::McpError, protocol::JsonRpcNotification, NotificationSender};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};
use tokio::sync::Mutex;
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

impl FromStr for LogLevel {
    type Err = McpError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "notice" => Ok(LogLevel::Notice),
            "warning" => Ok(LogLevel::Warning),
            "error" => Ok(LogLevel::Error),
            "critical" => Ok(LogLevel::Critical),
            "alert" => Ok(LogLevel::Alert),
            "emergency" => Ok(LogLevel::Emergency),
            _ => Err(McpError::InvalidRequest("Invalid log level".to_string())),
        }
    }
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::TRACE => LogLevel::Debug,
            Level::DEBUG => LogLevel::Debug,
            Level::INFO => LogLevel::Info,
            Level::WARN => LogLevel::Warning,
            Level::ERROR => LogLevel::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMessage {
    pub level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLevelRequest {
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingCapabilities {}

pub struct LoggingManager {
    current_level: LogLevel,
    notification_sender: Option<NotificationSender>,
    capabilities: LoggingCapabilities,
}

impl LoggingManager {
    pub fn new() -> Self {
        Self {
            current_level: LogLevel::Info,
            notification_sender: None,
            capabilities: LoggingCapabilities {},
        }
    }

    pub fn set_notification_sender(&mut self, sender: NotificationSender) {
        self.notification_sender = Some(sender);
    }

    pub async fn set_level(&mut self, level: String) -> Result<(), McpError> {
        self.current_level = match level.to_lowercase().as_str() {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "notice" => LogLevel::Notice,
            "warning" => LogLevel::Warning,
            "error" => LogLevel::Error,
            "critical" => LogLevel::Critical,
            "alert" => LogLevel::Alert,
            "emergency" => LogLevel::Emergency,
            _ => return Err(McpError::InvalidRequest("Invalid log level".to_string())),
        };
        Ok(())
    }

    pub async fn log(&self, message: LogMessage) -> Result<(), McpError> {
        if let Some(sender) = &self.notification_sender {
            // Skip internal debug logs
            if matches!(&message.level, LogLevel::Debug) {
                // Skip if logger is hyper or internal transport
                if let Some(logger) = &message.logger {
                    if logger.starts_with("hyper::") || 
                       logger.starts_with("mcp_rs::transport") {
                        return Ok(());
                    }
                }

                // Skip transport-related messages
                if message.data.get("message")
                    .and_then(|m| m.as_str())
                    .map_or(false, |m| {
                        m.contains("Broadcasting SSE message") || 
                        m.contains("Failed to broadcast message") ||
                        m.contains("-> ") ||
                        m.contains("<- ")
                    }) 
                {
                    return Ok(());
                }
            }

            let notification = JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "notifications/message".to_string(),
                params: Some(serde_json::to_value(message)?),
            };

            sender
                .tx
                .send(notification)
                .await
                .map_err(|e| McpError::InternalError(e.to_string()))?;
        }
        Ok(())
    }
}

// Custom tracing subscriber that forwards to LoggingManager
pub struct McpSubscriber {
    logging_manager: Arc<Mutex<LoggingManager>>,
}

impl McpSubscriber {
    pub fn new(logging_manager: Arc<Mutex<LoggingManager>>) -> Self {
        Self { logging_manager }
    }
}

impl<S: Subscriber> Layer<S> for McpSubscriber {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = LogLevel::from(*metadata.level());
        let logger = metadata.module_path().map(String::from);

        // Extract fields from the event
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);
        let data = visitor.0;

        // Create log message
        let message = LogMessage {
            level,
            logger,
            data,
        };

        // Send log message through the manager
        let logging_manager = self.logging_manager.clone();
        tokio::spawn(async move {
            let manager = logging_manager.lock().await;
            let _ = manager.log(message).await;
        });
    }
}

#[derive(Default)]
struct JsonVisitor(serde_json::Value);

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let value = format!("{:?}", value);
        if let serde_json::Value::Object(ref mut map) = self.0 {
            map.insert(field.name().to_string(), serde_json::Value::String(value));
        } else {
            self.0 = serde_json::json!({
                field.name(): value
            });
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if let serde_json::Value::Object(ref mut map) = self.0 {
            map.insert(
                field.name().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        } else {
            self.0 = serde_json::json!({
                field.name(): value
            });
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if let serde_json::Value::Object(ref mut map) = self.0 {
            map.insert(
                field.name().to_string(),
                serde_json::Value::Number(value.into()),
            );
        } else {
            self.0 = serde_json::json!({
                field.name(): value
            });
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if let serde_json::Value::Object(ref mut map) = self.0 {
            map.insert(
                field.name().to_string(),
                serde_json::Value::Number(value.into()),
            );
        } else {
            self.0 = serde_json::json!({
                field.name(): value
            });
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if let serde_json::Value::Object(ref mut map) = self.0 {
            map.insert(field.name().to_string(), serde_json::Value::Bool(value));
        } else {
            self.0 = serde_json::json!({
                field.name(): value
            });
        }
    }
}
