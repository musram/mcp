use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    // Send initialize request
    let init_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "resources": {
                    "subscribe": true,
                    "listChanged": true
                }
            }
        }
    });

    let init_msg = serde_json::to_string(&init_request).unwrap() + "\n";
    stdout.write_all(init_msg.as_bytes()).await.unwrap();

    // Send list resources request
    let list_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "resources/list",
        "params": {
            "cursor": null
        }
    });

    let list_msg = serde_json::to_string(&list_request).unwrap() + "\n";
    stdout.write_all(list_msg.as_bytes()).await.unwrap();

    // Read responses
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 {
            break;
        }
        println!("Received: {}", line);
        line.clear();
    }
}
