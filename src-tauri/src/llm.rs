//! OpenAI-compatible Chat Completions for prompt optimize.
//! Independent of image-gen providers. No conversation memory: every call is one request.

use crate::provider::{transport, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

const KEYRING_SERVICE: &str = "app.lutriui.desktop";
const KEYRING_USER: &str = "llm";
const MAX_USER_CHARS: usize = 100_000;
const MAX_URL_CHARS: usize = 2048;
const MAX_MODEL_CHARS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettings {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    pub base_url: String,
    pub model: String,
    pub has_key: bool,
    pub configured: bool,
}

static STORE: OnceLock<LlmStore> = OnceLock::new();

struct LlmStore {
    path: PathBuf,
    settings: RwLock<LlmSettings>,
}

fn coded(code: &str, message: impl Into<String>) -> ProviderError {
    ProviderError::coded(code, message)
}

fn templates() -> &'static Value {
    static TEMPLATES: OnceLock<Value> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        serde_json::from_str(include_str!("../../shared/prompt-optimize.json"))
            .expect("invalid prompt-optimize.json")
    })
}

fn template_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.to_string()),
        Value::Array(items) => {
            let parts: Vec<&str> = items
                .iter()
                .filter_map(Value::as_str)
                .filter(|part| !part.trim().is_empty())
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n\n"))
            }
        }
        _ => None,
    }
}

pub fn system_prompt(family: &str) -> String {
    let catalog = templates();
    let generic = template_text(&catalog["generic"]).unwrap_or_else(|| {
        "You rewrite image-generation prompts. Return only the rewritten prompt. This request is independent.".into()
    });
    match template_text(&catalog["families"][family]) {
        Some(extra) => format!("{generic}\n\n{extra}"),
        None => generic,
    }
}

pub fn validate_base_url(raw: &str) -> Result<String, ProviderError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(coded(
            "backend_llm_url_missing",
            "Enter the OpenAI-compatible base URL",
        ));
    }
    if trimmed.len() > MAX_URL_CHARS || trimmed.contains(['\r', '\n', '\0']) {
        return Err(coded(
            "backend_llm_url_invalid",
            "Enter a valid http(s) base URL",
        ));
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|_| {
        coded(
            "backend_llm_url_invalid",
            "Enter a valid http(s) base URL",
        )
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(coded(
            "backend_llm_url_invalid",
            "Enter a valid http(s) base URL",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(coded(
            "backend_llm_url_secrets",
            "Do not put the API key in the URL",
        ));
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

pub fn validate_model(raw: &str) -> Result<String, ProviderError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(coded(
            "backend_llm_model_missing",
            "Enter the Chat Completions model id",
        ));
    }
    if trimmed.len() > MAX_MODEL_CHARS || trimmed.contains(['\r', '\n', '\0']) {
        return Err(coded(
            "backend_llm_model_missing",
            "Enter the Chat Completions model id",
        ));
    }
    Ok(trimmed.to_string())
}

pub fn chat_completions_url(base: &str) -> Result<String, ProviderError> {
    let base = validate_base_url(base)?;
    if base.ends_with("/chat/completions") {
        Ok(base)
    } else {
        Ok(format!("{base}/chat/completions"))
    }
}

pub fn models_url(base: &str) -> Result<String, ProviderError> {
    let base = validate_base_url(base)?;
    if let Some(root) = base.strip_suffix("/chat/completions") {
        let root = root.trim_end_matches('/');
        Ok(format!("{root}/models"))
    } else {
        Ok(format!("{base}/models"))
    }
}

pub fn chat_body(model: &str, family: &str, user_content: &str) -> Value {
    json!({
        "model": model,
        "temperature": 0.3,
        "max_tokens": 4096,
        "messages": [
            { "role": "system", "content": system_prompt(family) },
            { "role": "user", "content": user_content },
        ]
    })
}

pub fn strip_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(inner) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(inner) = inner.strip_suffix("```") else {
        return trimmed;
    };
    match inner.split_once('\n') {
        Some((lang, body)) if lang.len() < 20 && !lang.contains(' ') => body.trim(),
        _ => inner.trim(),
    }
}

pub fn assistant_text(body: &Value) -> Result<String, ProviderError> {
    let message = body
        .pointer("/choices/0/message")
        .ok_or_else(empty_reply)?;
    if let Some(refusal) = message
        .get("refusal")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
    {
        return Err(coded("backend_llm_empty_reply", refusal.trim().to_string()));
    }
    let content = match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.as_str())
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let text = strip_fences(&content).to_string();
    if text.is_empty() {
        return Err(empty_reply());
    }
    Ok(text)
}

fn empty_reply() -> ProviderError {
    coded(
        "backend_llm_empty_reply",
        "The model returned an empty prompt",
    )
}

fn http_error(status: u16, body: &Value) -> ProviderError {
    if status == 401 || status == 403 {
        return coded(
            "backend_llm_unauthorized",
            "The API key is missing, invalid, or was rejected",
        )
        .with_status(status);
    }
    let upstream = body
        .pointer("/error/message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str);
    match upstream {
        Some(message) => coded(
            "backend_llm_request_failed",
            format!("Prompt optimize failed (HTTP {status}): {message}"),
        )
        .with_param("status", status)
        .with_status(status),
        None => coded(
            "backend_llm_request_failed",
            format!("Prompt optimize failed (HTTP {status})"),
        )
        .with_param("status", status)
        .with_status(status),
    }
}

pub fn read_settings_file(path: &Path) -> Result<LlmSettings, ProviderError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            coded(
                "backend_llm_settings_corrupt",
                "The prompt-optimize settings are corrupt",
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LlmSettings::default()),
        Err(_) => Err(coded(
            "backend_llm_settings_read_failed",
            "Couldn't read the prompt-optimize settings",
        )),
    }
}

pub fn write_settings_file(path: &Path, settings: &LlmSettings) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            coded(
                "backend_llm_settings_save_failed",
                "Couldn't save the prompt-optimize settings",
            )
        })?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(|_| {
        coded(
            "backend_llm_settings_save_failed",
            "Couldn't save the prompt-optimize settings",
        )
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|_| {
        coded(
            "backend_llm_settings_save_failed",
            "Couldn't save the prompt-optimize settings",
        )
    })?;
    std::fs::rename(tmp, path).map_err(|_| {
        coded(
            "backend_llm_settings_save_failed",
            "Couldn't save the prompt-optimize settings",
        )
    })
}

pub fn init(dir: &Path) -> Result<(), String> {
    let path = dir.join("llm.json");
    let settings = read_settings_file(&path)?;
    STORE
        .set(LlmStore {
            path,
            settings: RwLock::new(settings),
        })
        .map_err(|_| {
            coded(
                "backend_llm_settings_uninitialized",
                "Prompt-optimize settings were initialized twice",
            )
            .to_json()
        })?;
    Ok(())
}

fn store() -> Result<&'static LlmStore, ProviderError> {
    STORE.get().ok_or_else(|| {
        coded(
            "backend_llm_settings_uninitialized",
            "Prompt-optimize settings aren't initialized yet",
        )
    })
}

pub fn settings() -> Result<LlmSettings, ProviderError> {
    Ok(store()?
        .settings
        .read()
        .map_err(|_| {
            coded(
                "backend_llm_settings_unavailable",
                "Prompt-optimize settings are temporarily unavailable",
            )
        })?
        .clone())
}

pub fn save_settings(next: LlmSettings) -> Result<(), ProviderError> {
    let store = store()?;
    let mut guard = store.settings.write().map_err(|_| {
        coded(
            "backend_llm_settings_unavailable",
            "Prompt-optimize settings are temporarily unavailable",
        )
    })?;
    write_settings_file(&store.path, &next)?;
    *guard = next;
    Ok(())
}

fn credential_entry() -> Result<keyring::Entry, ProviderError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|_| {
        coded(
            "backend_credential_store_unavailable",
            "Couldn't access the system credential store",
        )
    })
}

pub fn stored_key() -> Option<String> {
    credential_entry()
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .filter(|value| !value.trim().is_empty())
}

pub fn save_key(key: &str) -> Result<(), ProviderError> {
    let key = key.trim();
    if key.is_empty() || key.contains(['\r', '\n']) {
        return Err(coded("backend_credential_invalid", "Enter a valid key"));
    }
    credential_entry()?.set_password(key).map_err(|_| {
        coded(
            "backend_credential_save_failed",
            "Couldn't save the key to the system credential store",
        )
    })
}

pub fn remove_key() -> Result<(), ProviderError> {
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(coded(
            "backend_credential_remove_failed",
            "Couldn't remove the key from the system credential store",
        )),
    }
}

pub fn status() -> Result<LlmStatus, ProviderError> {
    let settings = settings()?;
    let has_key = stored_key().is_some();
    let configured = has_key
        && !settings.base_url.trim().is_empty()
        && !settings.model.trim().is_empty();
    Ok(LlmStatus {
        base_url: settings.base_url,
        model: settings.model,
        has_key,
        configured,
    })
}

pub fn prepare_settings(base_url: String, model: String) -> Result<LlmSettings, ProviderError> {
    let base_url = base_url.trim();
    let model = model.trim();
    Ok(LlmSettings {
        base_url: if base_url.is_empty() {
            String::new()
        } else {
            validate_base_url(base_url)?
        },
        model: if model.is_empty() {
            String::new()
        } else {
            validate_model(model)?
        },
    })
}

fn require_user_content(user_content: &str) -> Result<&str, ProviderError> {
    let trimmed = user_content.trim();
    if trimmed.is_empty() {
        return Err(coded(
            "backend_llm_context_empty",
            "The optimize request has no draft context",
        ));
    }
    if trimmed.chars().count() > MAX_USER_CHARS {
        return Err(coded(
            "backend_llm_user_too_long",
            "The optimize context is too long",
        ));
    }
    Ok(trimmed)
}

pub async fn optimize_with(
    settings: &LlmSettings,
    key: &str,
    family: &str,
    user_content: &str,
) -> Result<String, ProviderError> {
    let user_content = require_user_content(user_content)?;
    let model = validate_model(&settings.model)?;
    let url = chat_completions_url(&settings.base_url)?;
    if key.trim().is_empty() {
        return Err(missing_key());
    }
    let client = transport::client_with_timeout(90)?;
    let response = client
        .post(&url)
        .bearer_auth(key.trim())
        .header("content-type", "application/json")
        .json(&chat_body(&model, family, user_content))
        .send()
        .await
        .map_err(|_| {
            coded(
                "backend_llm_network_failed",
                "Prompt optimize network request failed",
            )
        })?;
    let status = response.status().as_u16();
    let body: Value = response.json().await.map_err(|_| {
        coded(
            "backend_llm_unparseable",
            "The prompt-optimize response could not be parsed",
        )
    })?;
    if !(200..300).contains(&status) {
        return Err(http_error(status, &body));
    }
    assistant_text(&body)
}

pub async fn check_with(settings: &LlmSettings, key: &str) -> Result<(), ProviderError> {
    if key.trim().is_empty() {
        return Err(missing_key());
    }
    let url = models_url(&settings.base_url)?;
    let client = transport::client_with_timeout(30)?;
    let response = client
        .get(&url)
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(|_| {
            coded(
                "backend_llm_network_failed",
                "Prompt optimize network request failed",
            )
        })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.json().await.unwrap_or(Value::Null);
        return Err(http_error(status, &body));
    }
    Ok(())
}

pub async fn optimize(family: String, user_content: String) -> Result<String, ProviderError> {
    let key = stored_key().ok_or_else(missing_key)?;
    let settings = settings()?;
    optimize_with(&settings, &key, &family, &user_content).await
}

pub async fn check() -> Result<(), ProviderError> {
    let key = stored_key().ok_or_else(missing_key)?;
    let settings = settings()?;
    if settings.base_url.trim().is_empty() {
        return Err(coded(
            "backend_llm_url_missing",
            "Enter the OpenAI-compatible base URL",
        ));
    }
    check_with(&settings, &key).await
}

fn missing_key() -> ProviderError {
    coded(
        "backend_llm_key_missing",
        "Configure the OpenAI-compatible API key in Settings",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_templates_cover_catalog_and_unknown_falls_back() {
        let catalog: Value =
            serde_json::from_str(include_str!("../../shared/model-catalog.json")).unwrap();
        for family in catalog["families"].as_array().unwrap() {
            let id = family["id"].as_str().unwrap();
            assert!(
                template_text(&templates()["families"][id]).is_some(),
                "missing prompt-optimize template for {id}"
            );
            let prompt = system_prompt(id);
            assert!(prompt.contains("independent"));
            assert!(prompt.len() > system_prompt("not-a-real-family").len());
        }
        assert_eq!(
            system_prompt("not-a-real-family"),
            system_prompt("also-unknown")
        );
        assert!(system_prompt("flux").contains("FLUX.3 Image"));
        assert!(system_prompt("gpt").contains("GPT Image"));
        assert!(!system_prompt("not-a-real-family").contains("FLUX.3 Image"));
    }

    #[test]
    fn chat_body_is_one_independent_turn() {
        let body = chat_body("gpt-4.1-mini", "flux", "a red lantern");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "a red lantern");
        assert!(messages[0]["content"].as_str().unwrap().contains("FLUX.3 Image"));
        assert!(body.get("previous_response_id").is_none());
        assert!(body.get("conversation").is_none());
        assert!(body.get("conversation_id").is_none());
        assert!(body.get("store").is_none());
    }

    #[test]
    fn url_join_and_reject_embedded_secrets() {
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1/chat/completions").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            models_url("https://api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            validate_base_url("https://user:secret@host/v1")
                .unwrap_err()
                .code,
            "backend_llm_url_secrets"
        );
        assert!(validate_base_url("ftp://example.com/v1").is_err());
    }

    #[test]
    fn assistant_text_strips_fences_and_reads_parts() {
        let fenced = json!({"choices":[{"message":{"content":"```text\npolished prompt\n```"}}]});
        assert_eq!(assistant_text(&fenced).unwrap(), "polished prompt");
        let parts = json!({"choices":[{"message":{"content":[{"type":"text","text":"hello "},{"text":"world"}]}}]});
        assert_eq!(assistant_text(&parts).unwrap(), "hello world");
        assert!(assistant_text(&json!({"choices":[{"message":{"content":""}}]})).is_err());
    }

    #[test]
    fn settings_roundtrip_keeps_corrupt_path_out_of_keyring() {
        let dir = std::env::temp_dir().join(format!("lutri-llm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("llm.json");
        let saved = LlmSettings {
            base_url: "https://example.test/v1".into(),
            model: "demo-model".into(),
        };
        write_settings_file(&path, &saved).unwrap();
        assert_eq!(read_settings_file(&path).unwrap(), saved);
        let _ = std::fs::remove_dir_all(dir);
    }
}
