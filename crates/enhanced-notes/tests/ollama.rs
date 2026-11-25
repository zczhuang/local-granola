#![cfg(feature = "ollama")]
mod common;

use common::*;
use enhanced_notes::ollama::OllamaClient;
use enhanced_notes::{EnhanceError, enhance, output_json_schema};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn chat_reply(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "gemma4:26b-mlx",
        "message": {"role": "assistant", "content": content},
        "done": true
    }))
}

fn client(server: &MockServer) -> OllamaClient {
    OllamaClient::new(&server.uri(), "gemma4:26b-mlx", 16_384).unwrap()
}

#[tokio::test]
async fn posts_schema_format_with_deterministic_options_and_parses_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(chat_reply(
            &serde_json::to_string(&scripted_output()).unwrap(),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let result = enhance(input(raw_notes()), &client(&server)).await.unwrap();
    assert_eq!(result.metadata.engine.model, "gemma4:26b-mlx");
    assert_eq!(result.metadata.engine.num_ctx, 32_768);
    assert!(ai_items(&result.doc).len() >= 6);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["model"], "gemma4:26b-mlx");
    assert_eq!(body["stream"], false);
    assert_eq!(body["options"]["temperature"], 0);
    assert_eq!(body["options"]["num_ctx"], 16_384);
    assert_eq!(body["format"], output_json_schema());
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Never rewrite")
    );
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("a1: Q3 launch plan")
    );
}

#[tokio::test]
async fn invalid_json_is_retried_once_with_a_repair_message() {
    let server = MockServer::start().await;
    // First request gets garbage, the second (which carries the repair turn) gets valid JSON.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(|req: &Request| {
            let body: Value = req.body_json().unwrap();
            if body["messages"].as_array().unwrap().len() == 2 {
                chat_reply("here are your notes: {oops")
            } else {
                chat_reply(&serde_json::to_string(&scripted_output()).unwrap())
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let result = enhance(input(raw_notes()), &client(&server)).await.unwrap();
    assert!(!ai_items(&result.doc).is_empty());
    let requests = server.received_requests().await.unwrap();
    let retry: Value = requests[1].body_json().unwrap();
    let messages = retry["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2]["role"], "assistant");
    assert!(
        messages[3]["content"]
            .as_str()
            .unwrap()
            .contains("not valid JSON")
    );
}

#[tokio::test]
async fn two_invalid_answers_fail_cleanly() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(chat_reply("nope"))
        .expect(2)
        .mount(&server)
        .await;
    let err = enhance(input(raw_notes()), &client(&server))
        .await
        .unwrap_err();
    assert!(matches!(err, EnhanceError::BadModelOutput(_)), "{err}");
}

#[tokio::test]
async fn http_errors_surface_as_client_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"error\":\"model not found\"}"))
        .mount(&server)
        .await;
    let err = enhance(input(raw_notes()), &client(&server))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404") && err.to_string().contains("model not found"));
}

#[test]
fn non_loopback_hosts_are_refused_before_any_request() {
    assert!(OllamaClient::new("http://192.168.1.20:11434", "m", 4096).is_err());
    assert!(OllamaClient::new("https://ollama.example.com", "m", 4096).is_err());
}
