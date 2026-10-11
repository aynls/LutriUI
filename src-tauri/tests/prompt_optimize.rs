//! Local HTTP simulations for prompt optimize. Never calls image generate.
use lutriui_lib::llm::{check_with, optimize_with, LlmSettings};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn server(responses: Vec<(u16, Value)>) -> (String, thread::JoinHandle<Vec<(String, Value)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        for (status, body) in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "expected HTTP request did not arrive"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, length, offset) = loop {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    break (head, length, end + 4);
                }
            };
            while bytes.len() < offset + length {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let payload = if length > 0 {
                serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
            } else {
                Value::Null
            };
            requests.push((head, payload));
            let text = serde_json::to_string(&body).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
        requests
    });
    (url, handle)
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
async fn connection_check_hits_models_not_image_generate() {
    let (url, handle) = server(vec![(200, json!({"data":[{"id":"demo-model"}]}))]);
    check_with(&settings(&url), "test-only-key").await.unwrap();
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("GET /v1/models "));
    assert!(!requests[0].0.contains("generate"));
    assert!(!requests[0].0.contains("images"));
}
