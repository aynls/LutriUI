//! 共享网络与媒体处理。提交付费任务只发送一次；轮询只读取已经提交的任务。
use super::{OutputImage, ProviderError};
use base64::Engine;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

pub fn client() -> reqwest::Client {
    client_with_timeout(120).expect("HTTP client initialization failed")
}

/// 按超时秒数复用客户端。`reqwest::Client` 内部共享连接池，克隆成本很低。
pub fn client_with_timeout(secs: u64) -> Result<reqwest::Client, ProviderError> {
    static CLIENTS: OnceLock<Mutex<HashMap<u64, reqwest::Client>>> = OnceLock::new();
    let clients = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = clients.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(client) = guard.get(&secs) {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(|_| {
            ProviderError::coded(
                "backend_network_client_failed",
                "Couldn't initialize the network client",
            )
        })?;
    guard.insert(secs, client.clone());
    Ok(client)
}
pub fn key(provider: &str) -> Result<String, ProviderError> {
    super::configured_key(provider).ok_or_else(|| {
        ProviderError::coded(
            "backend_missing_api_key",
            format!("No {provider} API key is configured"),
        )
        .with_param("provider", provider)
        .with_hint("Set the key and its source in Settings.")
        .with_hint_code("backend_configure_key_source")
    })
}
pub async fn json(
    request: reqwest::RequestBuilder,
    provider: &str,
) -> Result<(u16, Value), ProviderError> {
    let (status, body, _) = json_with_retry_after(request, provider).await?;
    Ok((status, body))
}
async fn json_with_retry_after(
    request: reqwest::RequestBuilder,
    provider: &str,
) -> Result<(u16, Value, Option<Duration>), ProviderError> {
    let (status, body, retry_after, _) = json_with_metadata(request, provider).await?;
    Ok((status, body, retry_after))
}
pub fn send_failed(err: reqwest::Error, provider: &str) -> ProviderError {
    let (code, message) = if err.is_timeout() {
        (
            "backend_network_timeout",
            format!("{provider} timed out waiting for a response"),
        )
    } else {
        (
            "backend_network_failed",
            format!("{provider} network request failed"),
        )
    };
    ProviderError::coded(code, message)
        .with_param("provider", provider)
        .with_hint("The task may already have been accepted. The paid request will not be submitted again.")
        .with_hint_code("backend_network_maybe_accepted")
}

pub fn http_failed(status: u16, provider: &str, upstream: Option<&str>) -> ProviderError {
    if status == 401 || status == 403 {
        return ProviderError::coded(
            "backend_provider_unauthorized",
            format!("{provider} rejected the API key"),
        )
        .with_param("provider", provider)
        .with_status(status);
    }
    match upstream {
        Some(message) => ProviderError::http(status, format!("{provider}: {message}")),
        None => ProviderError::coded(
            "backend_request_failed",
            format!("{provider}: The request failed"),
        )
        .with_param("provider", provider)
        .with_status(status),
    }
}

pub async fn json_with_metadata(
    request: reqwest::RequestBuilder,
    provider: &str,
) -> Result<(u16, Value, Option<Duration>, Option<f64>), ProviderError> {
    let response = request.send().await.map_err(|err| send_failed(err, provider))?;
    let status = response.status().as_u16();
    let credits = response
        .headers()
        .get("X-Comfy-Credits-Used")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0);
    let retry_after = response
        .headers()
        .get("Retry-After")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.clamp(1, 60)));
    let body: Value = response.json().await.map_err(|_| {
        ProviderError::coded(
            "backend_unparseable_response",
            format!("{provider} returned a response that could not be parsed"),
        )
        .with_param("provider", provider)
    })?;
    if !(200..300).contains(&status) {
        let upstream = body
            .pointer("/error/message")
            .or_else(|| body.pointer("/errors/0/message"))
            .or_else(|| body.get("message"))
            .or_else(|| body.get("detail"))
            .and_then(Value::as_str);
        return Err(http_failed(status, provider, upstream));
    }
    Ok((status, body, retry_after, credits))
}
pub fn base64_image(b64: &str, mime: &str) -> Result<OutputImage, ProviderError> {
    let data_url = format!("data:{mime};base64,{b64}");
    super::parse_data_url(&data_url)?;
    Ok(OutputImage {
        details: None,
        data_url,
        media_type: mime.into(),
    })
}
pub async fn download_image(url: &str) -> Result<OutputImage, ProviderError> {
    if url.starts_with("data:") {
        let (mime, _) = super::parse_data_url(url)?;
        return Ok(OutputImage {
            details: None,
            data_url: url.into(),
            media_type: mime,
        });
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        ProviderError::coded(
            "backend_result_url_invalid",
            "The result image URL is invalid",
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ProviderError::coded(
            "backend_result_url_scheme",
            "The result image must use HTTP or HTTPS",
        ));
    }
    let mut response = client().get(parsed).send().await.map_err(|_| {
        ProviderError::coded(
            "backend_result_download_failed",
            "The image was generated, but downloading it failed",
        )
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        return Err(ProviderError::coded(
            "backend_result_download_http",
            format!("Downloading the result image failed (HTTP {status})"),
        )
        .with_param("status", status));
    }
    const MAX: usize = 64 * 1024 * 1024;
    if response.content_length().is_some_and(|n| n > MAX as u64) {
        return Err(ProviderError::coded(
            "backend_result_too_large",
            "The result image is larger than 64MiB",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ProviderError::coded(
            "backend_result_download_interrupted",
            "The result image download was interrupted",
        )
    })? {
        if bytes.len() + chunk.len() > MAX {
            return Err(ProviderError::coded(
                "backend_result_too_large",
                "The result image is larger than 64MiB",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let format = image::guess_format(&bytes).map_err(|_| {
        ProviderError::coded(
            "backend_result_not_image",
            "The downloaded result is not an image",
        )
    })?;
    let mime = match format {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::Png => "image/png",
        _ => {
            return Err(ProviderError::coded(
                "backend_result_format",
                "The result image format is not supported",
            ))
        }
    };
    base64_image(
        &base64::engine::general_purpose::STANDARD.encode(bytes),
        mime,
    )
}
pub async fn openai_images(
    body: &Value,
    default_format: &str,
) -> Result<Vec<OutputImage>, ProviderError> {
    let items = body["data"].as_array().ok_or_else(|| {
        ProviderError::coded(
            "backend_missing_image_data",
            "The response is missing image data",
        )
    })?;
    let mut images = Vec::new();
    for item in items {
        if let Some(b64) = item["b64_json"].as_str() {
            let fallback = format!("image/{default_format}");
            images.push(base64_image(
                b64,
                item["media_type"]
                    .as_str()
                    .or_else(|| item["mime_type"].as_str())
                    .unwrap_or(&fallback),
            )?);
        } else if let Some(url) = item["url"].as_str() {
            images.push(download_image(url).await?);
        }
    }
    if images.is_empty() {
        return Err(ProviderError::coded(
            "backend_no_image",
            "The response contains no image",
        ));
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_failed_distinguishes_auth_from_other_status() {
        assert_eq!(
            http_failed(401, "bfl", None).code,
            "backend_provider_unauthorized"
        );
        assert_eq!(
            http_failed(403, "runware", Some("nope")).code,
            "backend_provider_unauthorized"
        );
        assert_eq!(http_failed(500, "bfl", None).code, "backend_request_failed");
        assert!(http_failed(502, "comfy", Some("upstream")).code.is_empty());
    }
}
