//! Local HTTP simulations for prompt optimize. Never calls image generate.
mod common;

use lutriui_lib::llm::{check_with, optimize_with, LlmSettings};
use serde_json::{json, Value};
use std::net::TcpListener;

use common::server as http_server;

/// Optimize responses carry only status and a JSON body.
fn server(
    responses: Vec<(u16, Value)>,
) -> (String, std::thread::JoinHandle<Vec<(String, Value)>>) {
    http_server(responses, |_| String::new())
}

fn settings(url: &str) -> LlmSettings {
    LlmSettings {
        base_url: format!("{url}/v1"),
        model: "demo-model".into(),
    }
}

#[tokio::test]
async fn optimize_posts_one_independent_chat_turn() {
    let (url, handle) = server(vec![(
        200,
        json!({"choices":[{"message":{"content":"```\nrewritten lantern\n```"}}]}),
    )]);
    let out = optimize_with(
        &settings(&url),
        "test-only-key",
        "flux",
        "Task: edit\nCurrent prompt:\n<<<\na lantern\n>>>",
    )
    .await
    .unwrap();
    assert_eq!(out, "rewritten lantern");
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .0
        .starts_with("POST /v1/chat/completions "));
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("authorization: bearer test-only-key"));
    let messages = requests[0].1["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert!(messages[0]["content"].as_str().unwrap().contains("FLUX.3 Image"));
    assert_eq!(messages[1]["role"], "user");
    assert!(requests[0].1.get("previous_response_id").is_none());
    assert!(requests[0].1.get("conversation").is_none());
    assert!(requests[0].1.get("conversation_id").is_none());
    assert!(requests[0].1.get("store").is_none());
}

#[tokio::test]
async fn second_optimize_does_not_reuse_prior_messages() {
    let (url, handle) = server(vec![
        (
            200,
            json!({"choices":[{"message":{"content":"first"}}]}),
        ),
        (
            200,
            json!({"choices":[{"message":{"content":"second"}}]}),
        ),
    ]);
    let cfg = settings(&url);
    assert_eq!(
        optimize_with(&cfg, "k", "gpt", "first draft").await.unwrap(),
        "first"
    );
    assert_eq!(
        optimize_with(&cfg, "k", "qwen", "second draft")
            .await
            .unwrap(),
        "second"
    );
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 2);
    for (i, request) in requests.iter().enumerate() {
        let messages = request.1["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "turn {i} must not carry chat history");
        assert_eq!(messages[1]["content"], if i == 0 { "first draft" } else { "second draft" });
    }
    assert!(requests[0].1["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("GPT Image"));
    assert!(requests[1].1["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Qwen Image"));
}

#[tokio::test]
async fn missing_or_rejected_key_is_a_clear_error() {
    let err = optimize_with(
        &LlmSettings {
            base_url: "https://example.test/v1".into(),
            model: "demo".into(),
        },
        "",
        "flux",
        "context",
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "backend_llm_key_missing");

    let (url, handle) = server(vec![(
        401,
        json!({"error":{"message":"invalid api key"}}),
    )]);
    let err = optimize_with(&settings(&url), "bad", "flux", "context")
        .await
        .unwrap_err();
    assert_eq!(err.code, "backend_llm_unauthorized");
    handle.join().unwrap();
}

#[tokio::test]
async fn optimize_distinguishes_missing_completions_route() {
    let (url, handle) = server(vec![(404, json!({"error":{"message":"nope"}}))]);
    let err = optimize_with(&settings(&url), "k", "flux", "context")
        .await
        .unwrap_err();
    assert_eq!(err.code, "backend_llm_no_completions");
    handle.join().unwrap();
}

#[tokio::test]
async fn connection_check_hits_models_not_image_generate() {
    let (url, handle) = server(vec![(200, json!({"data":[{"id":"demo-model"}]}))]);
    check_with(&settings(&url), "test-only-key").await.unwrap();
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("GET /v1/models "));
    assert!(!requests[0].0.contains("generate"));
    assert!(!requests[0].0.contains("images"));
}

#[tokio::test]
async fn connection_check_distinguishes_auth_and_missing_models_route() {
    let (url, handle) = server(vec![(401, json!({"error":{"message":"bad key"}}))]);
    let err = check_with(&settings(&url), "bad").await.unwrap_err();
    assert_eq!(err.code, "backend_llm_unauthorized");
    handle.join().unwrap();

    let (url, handle) = server(vec![(404, json!({"error":{"message":"nope"}}))]);
    let err = check_with(&settings(&url), "ok").await.unwrap_err();
    assert_eq!(err.code, "backend_llm_check_no_models");
    handle.join().unwrap();

    let (url, handle) = server(vec![(500, json!({}))]);
    let err = check_with(&settings(&url), "ok").await.unwrap_err();
    assert_eq!(err.code, "backend_llm_check_failed");
    handle.join().unwrap();
}

#[tokio::test]
async fn connection_check_network_error_is_not_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let err = check_with(
        &LlmSettings {
            base_url: format!("http://{addr}/v1"),
            model: "demo".into(),
        },
        "k",
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "backend_llm_check_network");
}
