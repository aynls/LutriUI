//! OpenRouter Image API 适配器。
//!
//! 端点: POST https://openrouter.ai/api/v1/images（同步返回图片）。
//! 可发送的字段由模型目录中该路由的参数白名单决定。FLUX 区域写在提示词里，
//! 本适配器不另加结构化包围盒字段。

use serde_json::{json, Map, Value};

use super::{
    transport, GenerateOutput, GenerateRequest, OutputImage, ProviderError, ProviderResult,
};

pub const ENDPOINT: &str = "https://openrouter.ai/api/v1/images";

fn read_key() -> Result<String, ProviderError> {
    super::configured_key("openrouter").ok_or_else(|| {
        ProviderError::coded(
            "backend_missing_api_key",
            "No OpenRouter API key is configured",
        )
        .with_param("provider", "OpenRouter")
        .with_hint("Check the key source, environment variable, or saved key in Settings.")
        .with_hint_code("backend_check_key_settings")
    })
}

pub fn build_payload(req: &GenerateRequest) -> Result<Value, ProviderError> {
    crate::models::validate(req)?;
    let model = crate::models::resolve(req)?;
    if req.final_prompt.trim().is_empty() {
        return Err(ProviderError::coded(
            "backend_empty_prompt",
            "The prompt cannot be empty",
        ));
    }
    let mut p = Map::new();
    p.insert("model".into(), json!(model.wire_id()));
    p.insert("prompt".into(), json!(req.final_prompt));
    if let Some(r) = &req.params.resolution {
        p.insert("resolution".into(), json!(r));
    }
    if let Some(a) = &req.params.aspect_ratio {
        p.insert("aspect_ratio".into(), json!(a));
    }
    if let Some(s) = req.params.safety_tolerance {
        // OpenRouter 官方透传白名单中的唯一 BFL 参数
        p.insert("safety_tolerance".into(), json!(s));
    }
    for (wire, value) in [
        ("quality", req.params.quality.as_ref().map(|v| json!(v))),
        (
            "background",
            req.params.background.as_ref().map(|v| json!(v)),
        ),
        (
            "output_format",
            req.params.output_format.as_ref().map(|v| json!(v)),
        ),
        (
            "output_compression",
            req.params.output_compression.map(|v| json!(v)),
        ),
        (
            "moderation",
            req.params.moderation.as_ref().map(|v| json!(v)),
        ),
        ("seed", req.params.seed.map(|v| json!(v))),
        ("n", req.params.count.map(|v| json!(v))),
    ] {
        if let Some(v) = value {
            p.insert(wire.into(), v);
        }
    }
    if !req.images.is_empty() {
        for img in &req.images {
            super::parse_data_url(img)?;
        }
        let refs: Vec<Value> = req
            .images
            .iter()
            .map(|url| json!({ "type": "image_url", "image_url": { "url": url } }))
            .collect();
        p.insert("input_references".into(), Value::Array(refs));
    }
    Ok(Value::Object(p))
}

pub async fn generate(req: &GenerateRequest) -> ProviderResult {
    let key = read_key()?;
    let payload = build_payload(req)?;
    super::progress::report("waiting", None, None, None);

    let resp = transport::client_with_timeout(300)?
        .post(ENDPOINT)
        .bearer_auth(key)
        .header("X-Title", "LutriUI")
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                return crate::provider::transport::send_failed(e, "openrouter");
            }
            ProviderError::coded(
                "backend_openrouter_connect",
                format!("Couldn't connect to OpenRouter: {e}"),
            )
            .with_param("detail", e.to_string())
            .with_hint("Check the network connection. This synchronous paid call is not retried.")
            .with_hint_code("backend_sync_paid_no_retry")
        })?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

    if status != 200 {
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| truncate(&text, 300));
        let (hint, hint_code) = match status {
            401 => (
                Some("The API key is invalid or revoked. Check OPENROUTER_API_KEY.".to_string()),
                "backend_openrouter_key",
            ),
            402 => (
                Some(
                    "OpenRouter credits are too low. Add credits at openrouter.ai/credits."
                        .to_string(),
                ),
                "backend_openrouter_credits",
            ),
            413 => (
                Some(
                    "The request is too large. Lower the reference image size limit in advanced settings."
                        .to_string(),
                ),
                "backend_openrouter_too_large",
            ),
            429 => (
                Some("Too many requests, or the quota was exceeded. Try again later.".to_string()),
                "backend_openrouter_rate",
            ),
            500..=599 => (
                Some(
                    "OpenRouter or an upstream provider failed temporarily. The request was not retried."
                        .to_string(),
                ),
                "backend_openrouter_upstream",
            ),
            _ => (None, ""),
        };
        let auth = status == 401 || status == 403;
        return Err(ProviderError {
            status: Some(status),
            message: if auth {
                "OpenRouter rejected the API key".into()
            } else {
                message
            },
            hint,
            code: if auth {
                "backend_provider_unauthorized".into()
            } else {
                String::new()
            },
            hint_code: hint_code.to_string(),
            params: if auth {
                let mut params = Map::new();
                params.insert("provider".into(), json!("OpenRouter"));
                params
            } else {
                Map::new()
            },
        });
    }

    let data = body.get("data").and_then(|v| v.as_array()).ok_or_else(|| {
        ProviderError::coded(
            "backend_openrouter_missing_data",
            "The OpenRouter response is missing the data field",
        )
    })?;
    if data.is_empty() {
        return Err(ProviderError::coded(
            "backend_openrouter_no_image",
            "The OpenRouter response contains no image",
        ));
    }
    let mut images = Vec::new();
    for item in data {
        let b64 = item
            .get("b64_json")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::coded(
                    "backend_missing_b64",
                    "The response image is missing b64_json",
                )
            })?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| {
                ProviderError::coded(
                    "backend_base64",
                    format!("Couldn't decode the image base64: {e}"),
                )
                .with_param("detail", e.to_string())
            })?;
        let media_type = item
            .get("media_type")
            .and_then(|v| v.as_str())
            .unwrap_or("image/png")
            .to_string();
        images.push(OutputImage {
            details: None,
            data_url: format!(
                "data:{media_type};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ),
            media_type,
        });
    }

    Ok(GenerateOutput {
        provider: "openrouter".into(),
        model: req.model.clone(),
        final_prompt: req.final_prompt.clone(),
        images,
        usage: body.get("usage").cloned().unwrap_or(Value::Null),
        notes: Vec::new(),
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}
