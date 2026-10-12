//! Tauri 命令：前端唯一入口。密钥不经过任何命令的返回值。

use base64::Engine;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::gallery::{
    Board, GalleryBatchResult, GalleryFacets, GalleryItem, GalleryPage, GalleryPatch, GalleryQuery,
    ImportedImage, LibraryStats, MaintainOp, MaintenanceReport,
};
use crate::history::{GenerationFile, HistoryItem, McpSubmissionRecord};
use crate::library::{invalid, validate_id};
use crate::library_access::with_library;
use crate::mcp;
use crate::provider::{io_failed, GenerateOutput, GenerateRequest, ProviderError};
use crate::AppState;

/// 命令错误的 ProviderError JSON 结构：前端按 code 本地化，detail 等参数只做插值。
fn coded(code: &str, message: impl Into<String>) -> ProviderError {
    ProviderError::coded(code, message)
}

fn image_too_large() -> String {
    coded(
        "backend_image_too_large",
        "The image file exceeds the 64MB limit",
    )
    .into()
}

fn image_unrecognized() -> String {
    coded("backend_image_unrecognized", "Couldn't recognize the image").into()
}

fn image_area_64mp() -> String {
    coded(
        "backend_image_area_64mp",
        "The image area exceeds 64MP. Reduce it before importing",
    )
    .into()
}

fn uninitialized() -> String {
    coded(
        "backend_history_uninitialized",
        "History storage isn't initialized",
    )
    .into()
}

/// Read-only history lookup used by the UI bridge and MCP task reads.
#[tauri::command]
pub async fn history_get(app: tauri::AppHandle, id: String) -> Result<Option<HistoryItem>, String> {
    with_library(app, move |store| store.history_find(&id)).await
}

// ---------------------------------------------------------------------------
// MCP server settings and the WebView bridge. Sensitive configuration, the
// token reveal and bridge callbacks are restricted to the main window.
// ---------------------------------------------------------------------------

fn require_main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(coded(
            "backend_forbidden",
            "This action is only available to the main window",
        )
        .into())
    }
}

#[tauri::command]
pub async fn mcp_status(state: State<'_, AppState>) -> Result<mcp::McpStatus, String> {
    Ok(state.mcp()?.status())
}

#[tauri::command]
pub async fn mcp_configure(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    port: u16,
) -> Result<mcp::McpConfigureResult, String> {
    require_main(&window)?;
    state.mcp()?.configure(&app, enabled, port).await
}

#[tauri::command]
pub async fn mcp_connection(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<mcp::McpConnection, String> {
    require_main(&window)?;
    state.mcp()?.connection()
}

#[tauri::command]
pub async fn mcp_rotate_token(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<mcp::McpStatus, String> {
    require_main(&window)?;
    state.mcp()?.rotate_token(&app)
}

#[tauri::command]
pub async fn mcp_submission_get(
    app: tauri::AppHandle,
    idempotency_key: String,
) -> Result<Option<McpSubmissionRecord>, String> {
    with_library(app, move |store| store.mcp_submission_get(&idempotency_key)).await
}

#[tauri::command]
pub async fn mcp_bridge_register(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), String> {
    require_main(&window)?;
    if uuid::Uuid::parse_str(&instance_id).is_err() {
        return Err(coded("backend_invalid_args", "Invalid bridge instance id").into());
    }
    let mcp = state.mcp()?;
    mcp.bridge.register(instance_id);
    mcp.emit_status(&app);
    Ok(())
}

#[tauri::command]
pub async fn mcp_bridge_unregister(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), String> {
    require_main(&window)?;
    let mcp = state.mcp()?;
    mcp.bridge.unregister(&instance_id);
    mcp.emit_status(&app);
    Ok(())
}

#[tauri::command]
pub async fn mcp_bridge_claim(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    request_id: String,
    instance_id: String,
) -> Result<bool, String> {
    require_main(&window)?;
    Ok(state.mcp()?.bridge.claim(&request_id, &instance_id))
}

#[tauri::command]
pub async fn mcp_bridge_reply(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    request_id: String,
    instance_id: String,
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
) -> Result<(), String> {
    require_main(&window)?;
    let error = error.map(|value| mcp::ToolError {
        code: value["code"].as_str().unwrap_or("UI_ERROR").to_string(),
        message: value["message"]
            .as_str()
            .unwrap_or("The workspace action failed")
            .to_string(),
        details: value.get("details").cloned(),
    });
    state.mcp()?.bridge.reply(
        &request_id,
        &instance_id,
        mcp::BridgeReply { result, error },
    );
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub openrouter: bool,
    pub bfl: bool,
    pub comfy: bool,
    pub runware: bool,
    pub google: bool,
    pub ark: bool,
    pub byteplus: bool,
    pub xai: bool,
    pub qwencloud: bool,
    pub sources: std::collections::HashMap<String, String>,
    pub settings: std::collections::HashMap<String, crate::provider::CredentialSettings>,
    pub stored_keys: std::collections::HashMap<String, bool>,
}

#[tauri::command]
pub async fn provider_status() -> ProviderStatus {
    let mut sources = std::collections::HashMap::new();
    let mut settings = std::collections::HashMap::new();
    let mut stored_keys = std::collections::HashMap::new();
    for p in crate::models::provider_ids() {
        let config = crate::provider::key_settings(p);
        let source = if config.source == "manual" {
            "system"
        } else {
            "environment"
        };
        sources.insert(p.to_string(), source.to_string());
        settings.insert(p.to_string(), config);
        stored_keys.insert(p.to_string(), crate::provider::stored_key(p).is_some());
    }
    ProviderStatus {
        openrouter: crate::provider::configured_key("openrouter").is_some(),
        bfl: crate::provider::configured_key("bfl").is_some(),
        comfy: crate::provider::configured_key("comfy").is_some(),
        runware: crate::provider::configured_key("runware").is_some(),
        google: crate::provider::configured_key("google").is_some(),
        ark: crate::provider::configured_key("ark").is_some(),
        byteplus: crate::provider::configured_key("byteplus").is_some(),
        xai: crate::provider::configured_key("xai").is_some(),
        qwencloud: crate::provider::configured_key("qwencloud").is_some(),
        sources,
        settings,
        stored_keys,
    }
}

#[tauri::command]
pub async fn credential_configure(
    provider: String,
    source: String,
    env_name: String,
) -> Result<(), String> {
    crate::provider::save_key_settings(
        &provider,
        crate::provider::CredentialSettings {
            source,
            env_name: env_name.trim().into(),
        },
    )
}

#[tauri::command]
pub async fn credential_save(provider: String, key: String) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.contains(['\r', '\n']) {
        return Err(coded("backend_credential_invalid", "Enter a valid key").into());
    }
    crate::provider::credential_entry(&provider)?
        .set_password(key)
        .map_err(|_| {
            coded(
                "backend_credential_save_failed",
                "Couldn't save the key to the system credential store",
            )
            .into()
        })
}

#[tauri::command]
pub async fn credential_remove(provider: String) -> Result<(), String> {
    match crate::provider::credential_entry(&provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(coded(
            "backend_credential_remove_failed",
            "Couldn't remove the key from the system credential store",
        )
        .into()),
    }
}

#[tauri::command]
pub async fn credential_check(provider: String) -> Result<String, String> {
    let key = crate::provider::configured_key(&provider).ok_or_else(|| {
        coded(
            "backend_key_not_configured",
            "No key is configured for this provider",
        )
    })?;
    let client = crate::provider::transport::client_with_timeout(30)?;
    let req = match provider.as_str() {
        "openrouter" => client
            .get("https://openrouter.ai/api/v1/key")
            .bearer_auth(&key),
        "bfl" => client
            .get("https://api.bfl.ai/v1/credits")
            .header("x-key", &key),
        "xai" => client.get("https://api.x.ai/v1/models").bearer_auth(&key),
        "google" => client.get("https://generativelanguage.googleapis.com/v1/models").header("x-goog-api-key", &key),
        "ark" | "byteplus" | "qwencloud" => {
            return Err(coded(
                "backend_check_unsupported",
                "This provider has no confirmed free key check endpoint. Confirm model access by generating an image.",
            )
            .into())
        }
        "comfy" => client.get("https://cloud.comfy.org/api/user").header("X-API-Key", &key),
        "runware" => client.post("https://api.runware.ai/v1").bearer_auth(&key).json(&serde_json::json!([{
            "taskType":"accountManagement", "taskUUID":uuid::Uuid::new_v4().to_string(), "operation":"getDetails"
        }])),
        _ => return Err(crate::provider::unknown_provider(&provider)),
    };
    let response = req.send().await.map_err(|_| {
        coded(
            "backend_connection_failed",
            "Connection failed. Check the network and try again",
        )
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        return Err(coded(
            "backend_check_failed_http",
            format!("Key check failed (HTTP {status})"),
        )
        .with_param("status", status)
        .into());
    }
    if provider == "runware" {
        let body: serde_json::Value = response.json().await.map_err(|_| {
            coded(
                "backend_check_response_invalid",
                "The connection check response is invalid",
            )
        })?;
        if body["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
            || !body["data"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        {
            return Err(coded(
                "backend_runware_check_failed",
                "Runware connection check failed. Check the key and account permissions",
            )
            .into());
        }
    }
    // 返回稳定码，前端按当前语言展示。
    Ok("backend_check_verified".into())
}

#[tauri::command]
pub async fn llm_status() -> Result<crate::llm::LlmStatus, String> {
    crate::llm::status().map_err(Into::into)
}

#[tauri::command]
pub async fn llm_configure(base_url: String, model: String) -> Result<(), String> {
    crate::llm::save_settings(crate::llm::prepare_settings(base_url, model)?).map_err(Into::into)
}

#[tauri::command]
pub async fn llm_credential_save(key: String) -> Result<(), String> {
    crate::llm::save_key(&key).map_err(Into::into)
}

#[tauri::command]
pub async fn llm_credential_remove() -> Result<(), String> {
    crate::llm::remove_key().map_err(Into::into)
}

#[tauri::command]
pub async fn llm_check() -> Result<String, String> {
    crate::llm::check().await?;
    Ok("backend_check_verified".into())
}

#[tauri::command]
pub async fn prompt_optimize(
    state: State<'_, AppState>,
    family: String,
    user_content: String,
    request_id: String,
) -> Result<String, String> {
    let token = state.cancels.register(&request_id);
    let result = crate::cancel::run_cancellable(
        token,
        crate::llm::cancelled(),
        crate::llm::optimize(family, user_content),
    )
    .await;
    state.cancels.finish(&request_id);
    result.map_err(Into::into)
}

#[tauri::command]
pub async fn prompt_optimize_cancel(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), String> {
    state.cancels.cancel(&request_id);
    Ok(())
}

#[tauri::command]
pub async fn draft_load(app: tauri::AppHandle) -> Result<Option<serde_json::Value>, String> {
    with_library(app, |store| store.draft_load()).await
}

#[tauri::command]
pub async fn draft_save(app: tauri::AppHandle, draft: serde_json::Value) -> Result<(), String> {
    with_library(app, move |store| store.draft_save(&draft)).await
}

fn imported_bytes(bytes: Vec<u8>, name: String) -> Result<ImportedImage, String> {
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err(image_too_large());
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| image_unrecognized())?;
    let format = reader.format().ok_or_else(image_unrecognized)?;
    let (width, height) = reader.into_dimensions().map_err(|_| {
        coded(
            "backend_image_dimensions_unreadable",
            "Couldn't read the image dimensions",
        )
    })?;
    if u64::from(width) * u64::from(height) > 64_000_000 {
        return Err(image_area_64mp());
    }
    Ok(ImportedImage {
        data_url: format!(
            "data:{};base64,{}",
            format.to_mime_type(),
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        width,
        height,
        name,
        asset_id: None,
    })
}

/// 空名称表示默认名称；由前端按当前语言补全，后端不保存本地化文案。
#[tauri::command]
pub async fn import_url(url: String) -> Result<ImportedImage, String> {
    let parsed = reqwest::Url::parse(&url)
        .map_err(|_| coded("backend_url_invalid", "The image URL is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(coded(
            "backend_url_scheme",
            "Only http and https image URLs are supported",
        )
        .into());
    }
    let name = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();
    let client = crate::provider::transport::client_with_timeout(60)?;
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|_| coded("backend_download_failed", "Image download failed"))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        return Err(coded(
            "backend_download_http",
            format!("Image download failed (HTTP {status})"),
        )
        .with_param("status", status)
        .into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_IMPORT_BYTES)
    {
        return Err(image_too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        coded(
            "backend_download_interrupted",
            "The image download was interrupted",
        )
    })? {
        if bytes.len() as u64 + chunk.len() as u64 > MAX_IMPORT_BYTES {
            return Err(image_too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    imported_bytes(bytes, name)
}

#[tauri::command]
pub fn clipboard_image() -> Result<ImportedImage, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_unavailable())?;
    let im = clipboard.get_image().map_err(|_| {
        coded(
            "backend_clipboard_no_image",
            "The clipboard has no readable image",
        )
    })?;
    if im.width * im.height > 16_000_000 {
        return Err(coded("backend_clipboard_area_16mp", "The image area exceeds 16MP").into());
    }
    let rgba = image::RgbaImage::from_raw(im.width as u32, im.height as u32, im.bytes.into_owned())
        .ok_or_else(|| {
            coded(
                "backend_clipboard_image_invalid",
                "The clipboard image is invalid",
            )
        })?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|_| coded("backend_image_encode_failed", "Couldn't encode the image"))?;
    imported_bytes(bytes.into_inner(), String::new())
}

fn clipboard_unavailable() -> String {
    coded(
        "backend_clipboard_unavailable",
        "Couldn't access the clipboard",
    )
    .into()
}

#[tauri::command]
pub fn copy_image(data_url: String) -> Result<(), String> {
    let (_, bytes) = crate::provider::parse_data_url(&data_url)?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|_| coded("backend_image_decode_failed", "Couldn't decode the image"))?
        .into_rgba8();
    let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_unavailable())?;
    clipboard
        .set_image(arboard::ImageData {
            width: rgba.width() as usize,
            height: rgba.height() as usize,
            bytes: std::borrow::Cow::Owned(rgba.into_raw()),
        })
        .map_err(|_| coded("backend_clipboard_copy_failed", "Couldn't copy the image").into())
}

#[tauri::command]
pub async fn history_storage(app: tauri::AppHandle) -> Result<String, String> {
    with_library(app, |store| Ok(store.dir().to_string_lossy().into_owned())).await
}

#[tauri::command]
pub async fn library_migrate(
    app: tauri::AppHandle,
    path: String,
) -> Result<crate::storage::LibraryMigration, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::storage::migrate(&app, std::path::Path::new(&path))
    })
    .await
    .map_err(|e| {
        String::from(
            coded(
                "backend_library_failed",
                format!("Couldn't move the library: {e}"),
            )
            .with_param("detail", e.to_string()),
        )
    })?
}

#[tauri::command]
pub async fn library_maintain(
    app: tauri::AppHandle,
    operation: String,
) -> Result<MaintenanceReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _migration = state.gate.migrate()?;
        let mut guard = state.library.lock().unwrap_or_else(|e| e.into_inner());
        let store = guard.as_mut().ok_or_else(uninitialized)?;
        crate::storage::ensure_migratable(store)?;
        let op = match operation.as_str() {
            "check" => MaintainOp::Check,
            "rebuild_thumbnails" => MaintainOp::Rebuild,
            "cleanup_missing" => MaintainOp::Cleanup,
            _ => return Err(invalid("unknown maintenance operation")),
        };
        store.maintain(op)
    })
    .await
    .map_err(|e| {
        String::from(
            coded(
                "backend_library_failed",
                format!("Library maintenance failed: {e}"),
            )
            .with_param("detail", e.to_string()),
        )
    })?
}

#[tauri::command]
pub async fn generate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: GenerateRequest,
) -> Result<GenerateOutput, String> {
    let _generation = state.gate.generation()?;
    let request_id = request.request_id.clone();
    let history_id = request.history_id.clone().or_else(|| request_id.clone());
    let cancel_id = request_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let reporter: crate::provider::progress::Reporter = std::sync::Arc::new(
        move |phase, task_id, completed, status| {
            if let Some(id) = history_id.as_deref() {
                let state = app.state::<AppState>();
                let guard = state.library.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(store) = guard.as_ref() {
                    let _ = store.history_progress(id, phase, task_id);
                }
            }
            let _ = app.emit("generation-progress", serde_json::json!({"requestId": request_id, "phase": phase, "taskId": task_id, "completed": completed, "status": status}));
        },
    );
    let token = state.cancels.register(&cancel_id);
    let result = crate::cancel::run_cancellable(
        token,
        crate::cancel::cancelled("backend_generation_cancelled", "Cancelled"),
        crate::provider::progress::with_reporter(reporter, async {
            crate::provider::progress::report("submitting", None, None, None);
            crate::provider::dispatch(&request).await
        }),
    )
    .await;
    state.cancels.finish(&cancel_id);
    result.map_err(|e| e.to_json())
}

#[tauri::command]
pub async fn generate_cancel(state: State<'_, AppState>, request_id: String) -> Result<(), String> {
    state.cancels.cancel(&request_id);
    Ok(())
}

const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;

#[tauri::command]
pub async fn import_image(path: String) -> Result<ImportedImage, String> {
    let p = std::path::PathBuf::from(&path);
    let meta = std::fs::metadata(&p).map_err(|_| {
        coded(
            "backend_file_missing",
            format!("The file doesn't exist: {path}"),
        )
        .with_param("path", path.clone())
    })?;
    if !meta.is_file() {
        return Err(coded("backend_not_a_file", "The selected path is not a file").into());
    }
    if meta.len() > MAX_IMPORT_BYTES {
        return Err(image_too_large());
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        other => {
            return Err(coded(
                "backend_image_format_unsupported",
                format!("Unsupported image format: .{other} (supports png, jpg, webp, gif)"),
            )
            .with_param("ext", other)
            .into())
        }
    };
    let (width, height) = image::image_dimensions(&p).map_err(|_| {
        coded(
            "backend_image_dimensions_unreadable",
            "Couldn't read the image dimensions",
        )
    })?;
    let bytes = std::fs::read(&p).map_err(io_failed)?;
    let data_url = format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    let name = p
        .file_stem()
        .and_then(|e| e.to_str())
        .unwrap_or("image")
        .to_string();
    Ok(ImportedImage {
        data_url,
        width,
        height,
        name,
        asset_id: None,
    })
}

#[tauri::command]
pub async fn save_data_url(data_url: String, path: String) -> Result<String, String> {
    let p = std::path::PathBuf::from(&path);
    crate::history::write_data_url(&data_url, &p)?;
    Ok(path)
}

#[tauri::command]
pub async fn history_list(app: tauri::AppHandle) -> Result<Vec<HistoryItem>, String> {
    with_library(app, |store| store.history_list()).await
}

#[tauri::command]
pub async fn history_save(
    app: tauri::AppHandle,
    item: HistoryItem,
    files: Option<Vec<GenerationFile>>,
) -> Result<HistoryItem, String> {
    let files = files.unwrap_or_default();
    with_library(app, move |store| store.history_save(item, files)).await
}

#[tauri::command]
pub async fn history_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    with_library(app, move |store| store.history_delete(&id)).await
}

#[tauri::command]
pub async fn gallery_query(
    app: tauri::AppHandle,
    query: GalleryQuery,
) -> Result<GalleryPage, String> {
    with_library(app, move |store| store.gallery_query(query)).await
}

#[tauri::command]
pub async fn gallery_facets(app: tauri::AppHandle) -> Result<GalleryFacets, String> {
    with_library(app, |store| store.gallery_facets()).await
}

#[tauri::command]
pub async fn gallery_get(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<Vec<GalleryItem>, String> {
    with_library(app, move |store| store.gallery_get(&ids)).await
}

#[tauri::command]
pub async fn gallery_patch(
    app: tauri::AppHandle,
    ids: Vec<String>,
    patch: GalleryPatch,
) -> Result<Vec<GalleryItem>, String> {
    with_library(app, move |store| store.gallery_patch(&ids, patch)).await
}

#[tauri::command]
pub async fn gallery_delete(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<GalleryBatchResult, String> {
    with_library(app, move |store| store.gallery_delete(&ids)).await
}

#[tauri::command]
pub async fn gallery_export(
    app: tauri::AppHandle,
    ids: Vec<String>,
    directory: String,
) -> Result<GalleryBatchResult, String> {
    with_library(app, move |store| store.gallery_export(&ids, &directory)).await
}

#[tauri::command]
pub async fn gallery_import(
    app: tauri::AppHandle,
    data_url: String,
    name: String,
    source: String,
) -> Result<GalleryItem, String> {
    with_library(app, move |store| {
        store.gallery_import(&data_url, &name, &source)
    })
    .await
}

#[tauri::command]
pub async fn gallery_read(app: tauri::AppHandle, id: String) -> Result<ImportedImage, String> {
    with_library(app, move |store| store.gallery_read(&id)).await
}

#[tauri::command]
pub async fn board_list(app: tauri::AppHandle) -> Result<Vec<Board>, String> {
    with_library(app, |store| store.board_list()).await
}

#[tauri::command]
pub async fn board_create(app: tauri::AppHandle, name: String) -> Result<Board, String> {
    with_library(app, move |store| store.board_create(&name)).await
}

#[tauri::command]
pub async fn board_rename(
    app: tauri::AppHandle,
    id: String,
    name: String,
) -> Result<Board, String> {
    with_library(app, move |store| store.board_rename(&id, &name)).await
}

#[tauri::command]
pub async fn board_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    with_library(app, move |store| store.board_delete(&id)).await
}

#[tauri::command]
pub async fn board_add(
    app: tauri::AppHandle,
    board_id: String,
    ids: Vec<String>,
) -> Result<GalleryBatchResult, String> {
    with_library(app, move |store| store.board_add(&board_id, &ids)).await
}

#[tauri::command]
pub async fn board_remove(
    app: tauri::AppHandle,
    board_id: String,
    ids: Vec<String>,
) -> Result<GalleryBatchResult, String> {
    with_library(app, move |store| store.board_remove(&board_id, &ids)).await
}

#[tauri::command]
pub async fn board_move(
    app: tauri::AppHandle,
    from_board_id: String,
    to_board_id: String,
    ids: Vec<String>,
) -> Result<GalleryBatchResult, String> {
    with_library(app, move |store| {
        store.board_move(&from_board_id, &to_board_id, &ids)
    })
    .await
}

#[tauri::command]
pub async fn library_stats(app: tauri::AppHandle) -> Result<LibraryStats, String> {
    with_library(app, |store| store.library_stats()).await
}

/// lutri-thumb 协议的图片读取：严格单 UUID 路径，经操作门与库锁返回缩略图 BLOB。
pub fn thumbnail_bytes(app: &tauri::AppHandle, id: &str) -> Result<Vec<u8>, String> {
    let state = app.state::<AppState>();
    let _operation = state.gate.operation()?;
    validate_id(id)?;
    let guard = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let store = guard.as_ref().ok_or_else(uninitialized)?;
    store
        .thumbnail(id)?
        .ok_or_else(|| coded("backend_gallery_gone", "Image was removed from the gallery").into())
}
