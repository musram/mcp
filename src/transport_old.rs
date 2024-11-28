use std::future::Future;

use mcp_typegen::mcp_types::JsonrpcMessage;

/// Describes the minimal contract for a MCP transport that a client or server can communicate over.
pub trait Transport {
    /// Starts processing messages on the transport, including any connection steps that might need to be taken.
    /// 
    /// This method should only be called after callbacks are installed, or else messages may be lost.
    ///
    /// * NOTE: This method should not be called explicitly when using Client, Server, or Protocol classes, as they will implicitly call start()
    fn start(&self) -> impl Future<Output = ()>;

    /// Sends a JSON-RPC message (request or response).
    fn send(&self, message: JsonrpcMessage) -> impl Future<Output = ()>;

    /// Closes the connection
    fn close(&self) -> impl Future<Output = ()>;

    /// Callback for when the connection is closed for any reason.
    /// 
    /// This should be invoked when close() is called as well.
    fn on_close(&self, callback: Box<dyn Fn() -> ()>);

    /// Callback for when an error occurs.
    /// 
    /// Note that errors are not necessarily fatal; they are used for reporting any kind of exceptional condition out of band.
    fn on_error(&self, callback: Box<dyn Fn(String) -> ()>);

    /// Callback for when a message is received.
    fn on_message(&self, callback: Box<dyn Fn(JsonrpcMessage) -> ()>);

}