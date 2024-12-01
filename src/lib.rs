use protocol::JsonRpcNotification;

pub mod error;
pub mod server;
pub mod transport;
pub mod resource;
pub mod protocol;
pub mod tools;
pub mod prompts;
pub mod logging;
pub mod client;

#[derive(Debug, Clone)]
pub struct NotificationSender {
    pub tx: tokio::sync::mpsc::Sender<JsonRpcNotification>,
}
