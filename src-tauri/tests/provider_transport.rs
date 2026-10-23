//! Local HTTP simulations validate admission, auth, polling and single submission.
mod common;

use base64::Engine;
use lutriui_lib::provider::{ark, comfy, google, qwencloud, runware, xai, GenerateRequest};
use serde_json::{json, Value};

use common::server as http_server;

/// Comfy cases in this file read `X-Comfy-Credits-Used` and `Retry-After`.
/// Those headers stay here so the shared responder does not emit them for other suites.
fn server(responses: Vec<(u16, Value)>) -> (String, std::thread::JoinHandle<Vec<(String, Value)>>) {
    http_server(responses, |status| {
        let mut headers = String::from("Retry-After: 1\r\n");
        if status == 200 {
            headers.push_str("X-Comfy-Credits-Used: 12.75\r\n");
        }
        headers
    })
}

fn png() -> String {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(16, 16)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    )
}
fn req(provider: &str, count: u32) -> GenerateRequest {
    serde_json::from_value(json!({"provider":provider,"model":"gpt-image-2.5-flare","finalPrompt":"a simple icon","requestId":"50836053-a0ee-4cf5-b9d6-ae7c5d140ada","params":{"size":"1024x1024","outputFormat":"png","count":count}})).unwrap()
}
fn grok_req(provider: &str) -> GenerateRequest {
    serde_json::from_value(json!({"provider":provider,"model":"grok-imagine-image-2.0","finalPrompt":"an otter reading","params":{"quality":"medium","resolution":"1K"}})).unwrap()
}

#[tokio::test]
async fn grok_official_selects_json_generation_or_edit_endpoint_and_normalizes_cost() {
    for count in [0, 1, 5] {
        let im = png();
        let (url, handle) = server(vec![(
            200,
            json!({"data":[{"b64_json":im.split_once(',').unwrap().1,"mime_type":"image/png"}],"usage":{"cost_in_usd_ticks":600000000}}),
        )]);
        let mut r = grok_req("xai");
        r.images = vec![im; count];
        let out = xai::generate_at(&r, "test-only-key", &format!("{url}/v1/images"))
            .await
            .unwrap();
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].media_type, "image/png");
        assert_eq!(out.usage["cost"], 0.06);
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 1);
        let path = if count == 0 { "generations" } else { "edits" };
        assert!(requests[0]
            .0
            .starts_with(&format!("POST /v1/images/{path} ")));
        assert!(requests[0]
            .0
            .to_lowercase()
            .contains("authorization: bearer test-only-key"));
        assert!(requests[0]
            .0
            .to_lowercase()
            .contains("content-type: application/json"));
        assert_eq!(requests[0].1["n"], 1);
    }
}

#[tokio::test]
async fn grok_comfy_direct_result_uses_idempotency_and_header_credits_without_polling() {
    let (url, handle) = server(vec![(
        200,
        json!({"data":[{"url":png()}],"usage":{"cost_in_usd_ticks":600000000}}),
    )]);
    let out = comfy::generate_at(
        &grok_req("comfy"),
        "test-only-key",
        &format!("{url}/v2/models"),
    )
    .await
    .unwrap();
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.usage["credits"], 12.75);
    assert!(out.usage.get("cost").is_none());
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .0
        .starts_with("POST /v2/models/xai/grok-imagine-image-2.0 "));
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("x-api-key: test-only-key"));
    assert!(requests[0].0.to_lowercase().contains("idempotency-key:"));
}

#[tokio::test]
async fn grok_moderation_and_empty_results_fail_without_resubmission() {
    for provider in ["xai", "comfy"] {
        for (status, body, expected) in [
            (
                200,
                json!({"block_reason":"input moderation refused","usage":{}}),
                "input moderation refused",
            ),
            (200, json!({"data":[{}]}), "no image"),
            (
                429,
                json!({"error":{"message":"rate limited"}}),
                "rate limited",
            ),
        ] {
            let (url, handle) = server(vec![(status, body)]);
            let r = grok_req(provider);
            let result = if provider == "xai" {
                xai::generate_at(&r, "test-only-key", &url).await
            } else {
                comfy::generate_at(&r, "test-only-key", &url).await
            };
            assert!(result.unwrap_err().message.contains(expected));
            assert_eq!(handle.join().unwrap().len(), 1);
        }
    }
}
#[tokio::test]
async fn google_official_authenticates_in_header_and_sends_one_native_request() {
    for (model, resolution) in [
        ("gemini-3.1-flash-image", "512"),
        ("gemini-nano-banana-2.1", "4K"),
    ] {
        let image = png();
        let (url, handle) = server(vec![(
            200,
            json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":image.split_once(',').unwrap().1}}]}}],"usageMetadata":{"totalTokenCount":99}}),
        )]);
        let request:GenerateRequest=serde_json::from_value(json!({"provider":"google","model":model,"finalPrompt":"a simple icon","params":{"resolution":resolution,"aspectRatio":"1:1"}})).unwrap();
        let out = google::generate_at(&request, "test-only-key", &format!("{url}/v1/models"))
            .await
            .unwrap();
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .0
            .starts_with(&format!("POST /v1/models/{model}:generateContent ")));
        assert!(requests[0]
            .0
            .to_lowercase()
            .contains("x-goog-api-key: test-only-key"));
        assert!(!requests[0].0.lines().next().unwrap().contains("key"));
        assert_eq!(out.provider, "google");
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.usage["totalTokenCount"], 99);
        assert!(out.usage.get("cost").is_none());
        assert_eq!(out.model, model);
        assert_eq!(
            requests[0].1["generationConfig"]["responseFormat"]["image"]["imageSize"],
            resolution
        );
    }
}

#[tokio::test]
async fn seedream_official_regions_authenticate_and_normalize_results_without_fabricated_cost() {
    for provider in ["ark", "byteplus"] {
        let (url, handle) = server(vec![(
            200,
            json!({"data":[{"url":png(),"output_format":"png"}],"usage":{"generated_images":1}}),
        )]);
        let request:GenerateRequest=serde_json::from_value(json!({"provider":provider,"model":"seedream-5-pro","finalPrompt":"a simple icon","params":{"resolution":"1K"}})).unwrap();
        let out = ark::generate_at(
            &request,
            "test-only-key",
            &format!("{url}/api/v3/images/generations"),
        )
        .await
        .unwrap();
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .0
            .to_lowercase()
            .contains("authorization: bearer test-only-key"));
        assert_eq!(
            requests[0].1["model"],
            if provider == "ark" {
                "doubao-seedream-5-0-pro-260628"
            } else {
                "dola-seedream-5-0-pro-260628"
            }
        );
        assert_eq!(out.provider, provider);
        assert_eq!(out.images.len(), 1);
        assert!(out.usage.get("cost").is_none());
    }
}

#[tokio::test]
async fn official_image_admission_failures_are_not_retried() {
    for provider in ["google", "ark", "byteplus"] {
        let (url, handle) = server(vec![(429, json!({"error":{"message":"rate limited"}}))]);
        let request:GenerateRequest=serde_json::from_value(json!({"provider":provider,"model":if provider=="google"{"gemini-3-pro-image"}else{"seedream-5-pro"},"finalPrompt":"a simple icon","params":{"resolution":"1K"}})).unwrap();
        let result = if provider == "google" {
            google::generate_at(&request, "test-only-key", &url).await
        } else {
            ark::generate_at(&request, "test-only-key", &url).await
        };
        assert!(result.is_err());
        assert_eq!(handle.join().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn comfy_submits_once_and_collects_native_result_with_api_key() {
    let image = png();
    let b64 = image.split_once(',').unwrap().1;
    let (url, handle) = server(vec![
        (201, json!({"request_id":"test-job"})),
        (202, json!({"status":"IN_PROGRESS"})),
        (502, json!({"detail":"temporary gateway outage"})),
        (
            200,
            json!({"data":[{"b64_json":b64}],"usage":{"total_tokens":123}}),
        ),
    ]);
    let out = comfy::generate_at(
        &req("comfy", 1),
        "test-only-key",
        &format!("{url}/v2/models"),
    )
    .await
    .unwrap();
    let requests = handle.join().unwrap();
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.usage["credits"], 12.75);
    assert_eq!(requests.len(), 4);
    assert!(requests[0]
        .0
        .starts_with("POST /v2/models/openai/gpt-image-2.5-flare/requests "));
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("x-api-key: test-only-key"));
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("idempotency-key: 50836053-a0ee-4cf5-b9d6-ae7c5d140ada"));
    assert_eq!(requests[0].1["size"], "1024x1024");
    assert!(requests[1].0.starts_with("GET "));
    assert!(requests[2].0.starts_with("GET "));
    assert!(requests[3].0.starts_with("GET "));
}
#[tokio::test]
async fn runware_polls_one_task_and_counts_repeated_results_once() {
    let id = "50836053-a0ee-4cf5-b9d6-ae7c5d140ada";
    let image = png();
    let first = json!({"taskUUID":id,"imageUUID":"first","imageDataURI":image,"cost":0.2});
    let second = json!({"taskUUID":id,"imageUUID":"second","imageDataURI":image,"cost":0.3});
    let foreign =
        json!({"taskUUID":"other-job","imageUUID":"foreign","imageDataURI":image,"cost":9.0});
    let (url, handle) = server(vec![
        (200, json!({"data":[{"taskUUID":id}]})),
        (200, json!({"data":[first,foreign]})),
        (200, json!({"data":[first,second]})),
    ]);
    let out = runware::generate_at(&req("runware", 2), "test-only-key", &url)
        .await
        .unwrap();
    let requests = handle.join().unwrap();
    assert_eq!(out.images.len(), 2);
    assert_eq!(out.usage["cost"], 0.5);
    assert_eq!(requests[0].1[0]["taskType"], "imageInference");
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("authorization: bearer test-only-key"));
    assert_eq!(requests[1].1[0]["taskType"], "getResponse");
    assert_eq!(requests[2].1[0]["taskUUID"], id);
    assert_eq!(
        requests
            .iter()
            .filter(|(_, body)| body[0]["taskType"] == "imageInference")
            .count(),
        1
    );
}
#[tokio::test]
async fn runware_polling_rejection_keeps_provider_message_without_resubmitting() {
    let id = "50836053-a0ee-4cf5-b9d6-ae7c5d140ada";
    let (url, handle) = server(vec![
        (200, json!({"data":[{"taskUUID":id}]})),
        (
            400,
            json!({"errors":[{"taskUUID":id,"code":"invalidProviderContent","message":"Rejected by Google's content moderation system."}]}),
        ),
    ]);
    let error = runware::generate_at(&req("runware", 1), "test-only-key", &url)
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(400));
    assert!(error.message.contains("Google's content moderation"));
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1[0]["taskType"], "imageInference");
    assert_eq!(requests[1].1[0]["taskType"], "getResponse");
}

#[tokio::test]
async fn admission_error_does_not_resubmit_paid_generation() {
    let (url, handle) = server(vec![(
        402,
        json!({"error":{"message":"not enough credits"}}),
    )]);
    let error = comfy::generate_at(&req("comfy", 1), "test-only-key", &url)
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(402));
    assert_eq!(handle.join().unwrap().len(), 1);
}
#[tokio::test]
async fn google_keeps_candidate_sources_and_summaries_without_saving_thought_images() {
    let image = png();
    let body = json!({"candidates":[{"content":{"parts":[
        {"thought":true,"text":"Provider summary"},
        {"thought":true,"inlineData":{"mimeType":"image/png","data":image.split_once(',').unwrap().1}},
        {"text":"Image explanation"},
        {"inlineData":{"mimeType":"image/png","data":image.split_once(',').unwrap().1}}
    ]},"groundingMetadata":{
        "webSearchQueries":["flowers"],"imageSearchQueries":["flower composition"],
        "searchEntryPoint":{"renderedContent":"<div>Google Search</div>"},
        "groundingChunks":[{"web":{"uri":"https://example.org/article","title":"Article"}},{"image":{"uri":"https://example.org/photo-page","image_uri":"https://example.org/raw.png"}},{"web":{"uri":"javascript:bad()","title":"Invalid"}}]
    }},{"content":{"parts":[{"text":"Second candidate"},{"inlineData":{"mimeType":"image/png","data":image.split_once(',').unwrap().1}}]}}]});
    let (url, handle) = server(vec![(200, body)]);
    let request = req_google_details();
    let out = google::generate_at(&request, "test-only-key", &url)
        .await
        .unwrap();
    assert_eq!(handle.join().unwrap().len(), 1);
    assert_eq!(out.images.len(), 2);
    let first = out.images[0].details.as_ref().unwrap();
    assert_eq!(first.text, "Image explanation");
    assert_eq!(first.thoughts, "Provider summary");
    assert_eq!(first.sources.len(), 2);
    assert_eq!(first.sources[1].url, "https://example.org/photo-page");
    assert_eq!(first.sources[1].kind, "image");
    assert_eq!(first.search_queries, vec!["flowers", "flower composition"]);
    assert_eq!(
        first.search_html.as_deref(),
        Some("<div>Google Search</div>")
    );
    let second = out.images[1].details.as_ref().unwrap();
    assert_eq!(second.text, "Second candidate");
    assert!(second.sources.is_empty());
}
#[tokio::test]
async fn qwencloud_posts_once_and_keeps_the_dashscope_image() {
    let image = png();
    let (url, handle) = server(vec![(
        200,
        json!({
            "output": {"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": [{"image": image}]}}]},
            "usage": {"image_count": 1, "width": 1024, "height": 1024},
            "request_id": "req-1"
        }),
    )]);
    let request: GenerateRequest = serde_json::from_value(json!({
        "provider": "qwencloud",
        "model": "qwen-image-2.1-pro",
        "finalPrompt": "a clear poster",
        "params": {"width": 1024, "height": 1024, "promptExtend": true, "promptExtendMode": "direct", "enableThinking": true, "watermark": false, "count": 1}
    }))
    .unwrap();
    let out = qwencloud::generate_at(&request, "test-only-key", &url)
        .await
        .unwrap();
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("POST / "));
    assert!(requests[0]
        .0
        .to_lowercase()
        .contains("authorization: bearer test-only-key"));
    assert_eq!(requests[0].1["model"], "qwen-image-2.1-pro");
    assert_eq!(requests[0].1["parameters"]["size"], "1024*1024");
    assert_eq!(requests[0].1["parameters"]["enable_thinking"], true);
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.usage["requestId"], "req-1");
    assert_eq!(out.usage["image_count"], 1);
    assert!(out.usage.get("cost").is_none());

    let (url, handle) = server(vec![(
        400,
        json!({"code":"InvalidParameter","message":"bad size"}),
    )]);
    let err = qwencloud::generate_at(&request, "test-only-key", &url)
        .await
        .unwrap_err();
    assert_eq!(err.status, Some(400));
    assert!(err.message.contains("bad size"));
    assert_eq!(handle.join().unwrap().len(), 1);
}
fn req_google_details() -> GenerateRequest {
    serde_json::from_value(json!({"provider":"google","model":"gemini-nano-banana-2.1","finalPrompt":"a flower","params":{"thinkingLevel":"medium","includeThoughts":true,"searchMode":"web_images","responseText":true}})).unwrap()
}
