mod cancel;
mod commands;
mod gallery;
mod history;
pub mod library;
mod library_access;
mod library_sql;
pub mod llm;
pub mod mcp;
pub mod models;
pub mod provider;
pub mod storage;

use std::sync::Mutex;

use tauri::Manager;

pub struct AppState {
    pub library: Mutex<Option<library::LibraryStore>>,
    pub gate: storage::StorageGate,
    pub mcp: std::sync::OnceLock<mcp::Mcp>,
    pub cancels: cancel::InflightCancels,
}

impl AppState {
    pub fn mcp(&self) -> Result<&mcp::Mcp, String> {
        self.mcp.get().ok_or_else(|| {
            String::from(crate::provider::ProviderError::coded(
                "backend_mcp_unavailable",
                "The MCP server isn't initialized",
            ))
        })
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library: Mutex::new(None),
            gate: storage::StorageGate::default(),
            mcp: std::sync::OnceLock::new(),
            cancels: cancel::InflightCancels::default(),
        }
    }
}

fn thumbnail_error(status: u16) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .body(Vec::new())
        .unwrap()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_opener::Builder::default()
                .open_js_links_on_click(false)
                .build(),
        )
        .manage(AppState::default())
        .register_asynchronous_uri_scheme_protocol("lutri-thumb", |ctx, request, responder| {
            let path = request.uri().path().trim_matches('/').to_string();
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let uuid_ok = uuid::Uuid::parse_str(&path).is_ok();
                let response = if !uuid_ok {
                    thumbnail_error(400)
                } else {
                    match commands::thumbnail_bytes(&app, &path) {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header(tauri::http::header::CONTENT_TYPE, "image/png")
                            .header(tauri::http::header::CACHE_CONTROL, "no-store")
                            .body(bytes)
                            .unwrap(),
                        Err(e)
                            if e.contains("bad_request")
                                || e.contains("backend_gallery_id_invalid") =>
                        {
                            thumbnail_error(400)
                        }
                        Err(e) if e.contains("backend_gallery_gone") => thumbnail_error(404),
                        Err(_) => thumbnail_error(503),
                    }
                };
                responder.respond(response);
            });
        })
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            let dir = storage::resolve_root(&app_data)
                .map_err(|e| format!("Couldn't initialize history storage: {e}"))?;
            let store = library::LibraryStore::new(dir.clone())
                .map_err(|e| format!("Couldn't initialize history storage: {e}"))?;
            // Register the existing, resolved image roots before the WebView reads assets.
            // Keep credentials, drafts and metadata outside the asset protocol scope.
            let assets = app.asset_protocol_scope();
            assets.allow_directory(dir.join("images"), true)?;
            assets.allow_directory(dir.join("gallery/images"), true)?;
            provider::init_key_settings(&app_data)?;
            llm::init(&app_data)?;
            *app.state::<AppState>().library.lock().unwrap() = Some(store);
            let state = app.state::<AppState>();
            state
                .mcp
                .set(mcp::Mcp::new(app_data))
                .map_err(|_| "Couldn't initialize the MCP server state")?;
            // Auto-start only when the saved setting is enabled; startup errors
            // surface in the settings page instead of aborting the app.
            state.mcp()?.setup(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::provider_status,
            commands::credential_save,
            commands::credential_remove,
            commands::credential_check,
            commands::credential_configure,
            commands::llm_status,
            commands::llm_configure,
            commands::llm_credential_save,
            commands::llm_credential_remove,
            commands::llm_check,
            commands::prompt_optimize,
            commands::prompt_optimize_cancel,
            commands::draft_load,
            commands::draft_save,
            commands::import_url,
            commands::clipboard_image,
            commands::copy_image,
            commands::history_storage,
            commands::library_migrate,
            commands::library_maintain,
            commands::library_stats,
            commands::generate,
            commands::generate_cancel,
            commands::import_image,
            commands::save_data_url,
            commands::history_list,
            commands::history_save,
            commands::history_delete,
            commands::gallery_query,
            commands::gallery_facets,
            commands::gallery_get,
            commands::gallery_patch,
            commands::gallery_import,
            commands::gallery_read,
            commands::gallery_delete,
            commands::gallery_export,
            commands::board_list,
            commands::board_create,
            commands::board_rename,
            commands::board_delete,
            commands::board_add,
            commands::board_remove,
            commands::board_move,
            commands::history_get,
            commands::mcp_status,
            commands::mcp_configure,
            commands::mcp_connection,
            commands::mcp_rotate_token,
            commands::mcp_submission_get,
            commands::mcp_bridge_register,
            commands::mcp_bridge_unregister,
            commands::mcp_bridge_claim,
            commands::mcp_bridge_reply,
        ])
        .build(tauri::generate_context!())
        .expect("LutriUI failed to start")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Ok(mcp) = app.state::<AppState>().mcp() {
                    mcp.shutdown();
                }
            }
        });
}
