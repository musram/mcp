use futures::StreamExt;
use reqwest::Client;
use reqwest_eventsource::{Event, EventSource};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn test_sse_client() {
    // Create EventSource client
    let url = "http://127.0.0.1:3000/events";
    let request_builder = Client::new().get(url);
    let mut event_source = EventSource::new(request_builder).unwrap();

    // Send a test request using regular HTTP client
    let client = Client::new();
    let response = client.post("http://127.0.0.1:3000/rpc")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/list",
            "params": { "cursor": null }
        }))
        .send()
        .await
        .unwrap();

    println!("Response: {:?}", response.text().await.unwrap());

    // Listen for SSE events
    while let Some(event) = event_source.next().await {
        match event {
            Ok(Event::Message(message)) => {
                println!("Received: {}", message.data);
            }
            Ok(Event::Open) => {
                println!("Connection opened");
            }
            Err(error) => {
                println!("Error: {}", error);
                break;
            }
        }
    }
}
