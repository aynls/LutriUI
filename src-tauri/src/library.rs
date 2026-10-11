use crate::library_sql;
use crate::provider::ProviderError;
use rusqlite::Connection;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

pub use crate::gallery::{
    BatchFailure, Board, GalleryBatchResult, GalleryFacets, GalleryItem, GalleryPage, GalleryPatch,
    GalleryQuery, ImportedImage, LibraryStats, MaintainOp, MaintenanceReport,
};
pub use crate::history::{GenerationFile, HistoryItem};

pub(crate) const DB_FILE: &str = "library.sqlite3";
pub(crate) const THUMB_SIZE: u32 = 384;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_DATA_URL: usize = 90_000_000;
const MAX_PIXELS: u64 = 64_000_000;
pub(crate) const SUPPORTED_VERSION: i64 = 3;

pub struct LibraryStore {
    pub(crate) root: PathBuf,
    pub(crate) conn: Connection,
}

impl LibraryStore {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        let images = root.join("images");
        let gallery = root.join("gallery");
        let gallery_images = gallery.join("images");
        for dir in [&root, &images, &gallery, &gallery_images] {
            match plain_metadata(dir) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(store_failed(
                        "inspect library directory",
                        format!("{} isn't a directory", dir.display()),
                    ))
                }
                Err(e) if is_genuine_missing(&e) => {
                    fs::create_dir_all(dir)
                        .map_err(|e| store_failed("create library directory", e))?;
                }
                Err(e) => return Err(store_failed("inspect library directory", e)),
            }
            check_directory(dir)?;
        }
        for suffix in ["", "-wal", "-shm"] {
            let file = root.join(format!("{DB_FILE}{suffix}"));
            match plain_metadata(&file) {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => {
                    return Err(store_failed(
                        "inspect library database",
                        format!("{} isn't a file", file.display()),
                    ))
                }
                Err(e) if is_genuine_missing(&e) => {}
                Err(e) => return Err(store_failed("inspect library database", e)),
            }
        }
        let conn = Connection::open(root.join(DB_FILE))
            .map_err(|e| store_failed("open library database", e))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| store_failed("configure library database", e))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
            .map_err(|e| store_failed("configure library database", e))?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| store_failed("inspect library database", e))?;
        if version != 0 && version != 1 && version != 2 && version != SUPPORTED_VERSION {
            return Err(String::from(
                ProviderError::coded(
                    "backend_library_unsupported",
                    "Library version {version} isn't supported",
                )
                .with_param("version", version.to_string()),
            ));
        }
        if version == 0 {
            conn.execute_batch(library_sql::SCHEMA)
                .map_err(|e| store_failed("initialize library database", e))?;
        } else {
            conn.execute_batch("PRAGMA journal_mode = WAL;")
                .map_err(|e| store_failed("configure library database", e))?;
            if version < SUPPORTED_VERSION {
                migrate_library(&conn).map_err(|e| store_failed("upgrade library database", e))?;
            }
        }
        // Feature tables outside user_version run for both new and existing libraries.
        conn.execute_batch(library_sql::MCP_SCHEMA)
            .map_err(|e| store_failed("initialize library database", e))?;
        let mut store = Self { root, conn };
        store
            .retry_pending_deletes()
            .map_err(|e| store_failed("clean up deleted images", e))?;
        store
            .recover_interrupted()
            .map_err(|e| store_failed("recover history", e))?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.root
    }

    pub(crate) fn images_root(&self) -> PathBuf {
        self.root.join("gallery").join("images")
    }

    pub(crate) fn asset_dir(&self, id: &str) -> PathBuf {
        self.images_root().join(id)
    }

    pub(crate) fn asset_path(&self, id: &str, mime: &str) -> PathBuf {
        self.asset_dir(id)
            .join(format!("original.{}", extension(mime)))
    }

    pub(crate) fn input_root(&self) -> PathBuf {
        self.root.join("images")
    }

    pub(crate) fn owned_relative(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        Some(rel.to_string_lossy().replace('\\', "/"))
    }

    pub(crate) fn owned_path(&self, relative: &str) -> Option<PathBuf> {
        let path = self.root.join(relative);
        let rel = path.strip_prefix(&self.root).ok()?;
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        Some(path)
    }

    /// Walks every ancestor of `dir` from the library root and requires a plain directory.
    pub(crate) fn check_dir_chain(&self, dir: &Path) -> Result<(), String> {
        check_directory(&self.root)?;
        let relative = dir
            .strip_prefix(&self.root)
            .map_err(|_| invalid("path outside the library"))?;
        let mut current = self.root.clone();
        for component in relative.components() {
            match component {
                Component::Normal(_) => current.push(component),
                _ => return Err(invalid("path outside the library")),
            }
            let meta = plain_metadata(&current)
                .map_err(|e| io_failed("inspect library directories", e))?;
            if !meta.is_dir() {
                return Err(io_failed(
                    "inspect library directories",
                    format!("{} isn't a directory", current.display()),
                ));
            }
        }
        Ok(())
    }
}

fn migrate_library(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(library_sql::BOARDS_SCHEMA)?;
    drop_favorite_column(conn)?;
    conn.pragma_update(None, "user_version", SUPPORTED_VERSION)?;
    Ok(())
}

fn drop_favorite_column(conn: &Connection) -> Result<(), rusqlite::Error> {
    let has_favorite = {
        let mut stmt = conn.prepare("PRAGMA table_info(assets)")?;
        let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
        let mut found = false;
        for name in names {
            if name? == "favorite" {
                found = true;
                break;
            }
        }
        found
    };
    if has_favorite {
        conn.execute_batch(
            "DROP INDEX IF EXISTS assets_favorite_date; ALTER TABLE assets DROP COLUMN favorite;",
        )?;
    }
    Ok(())
}

pub(crate) fn store_failed(context: &str, e: impl std::fmt::Display) -> String {
    String::from(
        ProviderError::coded("backend_library_failed", format!("Library {context}: {e}"))
            .with_param("detail", format!("{context}: {e}")),
    )
}

pub(crate) fn db_failed(context: &str, e: impl std::fmt::Display) -> String {
    String::from(
        ProviderError::coded(
            "backend_library_database",
            format!("Library database {context}: {e}"),
        )
        .with_param("detail", format!("{context}: {e}")),
    )
}

pub(crate) fn io_failed(context: &str, e: impl std::fmt::Display) -> String {
    String::from(
        ProviderError::coded("backend_library_io", format!("Library file {context}: {e}"))
            .with_param("detail", format!("{context}: {e}")),
    )
}

pub(crate) fn invalid(reason: impl std::fmt::Display) -> String {
    String::from(
        ProviderError::coded("backend_library_invalid", "Invalid library request")
            .with_param("detail", reason.to_string()),
    )
}

pub(crate) fn validate_id(id: &str) -> Result<(), String> {
    if Uuid::parse_str(id).is_err() {
        return Err(String::from(ProviderError::coded(
            "backend_gallery_id_invalid",
            "Invalid image id",
        )));
    }
    Ok(())
}

pub(crate) fn validate_history_id(id: &str) -> Result<(), String> {
    let safe = !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        return Err(String::from(ProviderError::coded(
            "backend_history_id_invalid",
            "Invalid history id",
        )));
    }
    Ok(())
}

pub(crate) fn validate_ids(ids: &[String]) -> Result<(), String> {
    if ids.is_empty() || ids.len() > 500 {
        return Err(String::from(ProviderError::coded(
            "backend_gallery_ids_invalid",
            "Image id list must contain 1-500 ids",
        )));
    }
    for id in ids {
        validate_id(id)?;
    }
    Ok(())
}

pub(crate) fn extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

pub(crate) fn sanitize_name(name: &str) -> Result<String, String> {
    let name = name.trim().replace(['\\', '/', ':', '\0'], "_");
    if name.is_empty() || name == "." || name == ".." || name.len() > 100 {
        return Err(String::from(ProviderError::coded(
            "backend_history_name_invalid",
            "Invalid file name",
        )));
    }
    Ok(name)
}

fn image_too_large() -> String {
    String::from(ProviderError::coded(
        "backend_image_too_large",
        "Image exceeds the size limit",
    ))
}

/// Decodes a bounded `data:<mime>;base64` payload and enforces the image MIME whitelist.
pub(crate) fn parse_image_data(data_url: &str) -> Result<(String, Vec<u8>), String> {
    if data_url.len() > MAX_DATA_URL {
        return Err(image_too_large());
    }
    let (mime, payload) = data_url
        .split_once(",")
        .ok_or_else(|| invalid("image data isn't a data url"))?;
    let mime = mime
        .strip_prefix("data:")
        .and_then(|s| s.strip_suffix(";base64"))
        .ok_or_else(|| invalid("image data isn't a base64 data url"))?
        .to_string();
    if !matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return Err(invalid("unsupported image format"));
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| invalid(e))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(image_too_large());
    }
    Ok((mime, bytes))
}

/// Validates the encoded size, format/MIME match and dimensions without decoding pixels.
pub(crate) fn check_image_header(mime: &str, bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(image_too_large());
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| invalid(e))?;
    let format = reader
        .format()
        .ok_or_else(|| invalid("unsupported image"))?;
    let expected = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Gif => "image/gif",
        _ => return Err(invalid("unsupported image format")),
    };
    if expected != mime {
        return Err(invalid("image format doesn't match data url"));
    }
    let (width, height) = reader.into_dimensions().map_err(|e| invalid(e))?;
    if width == 0 || height == 0 {
        return Err(invalid("image has no pixels"));
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(image_too_large());
    }
    Ok((width, height))
}

pub(crate) fn decode_image_data(data_url: &str) -> Result<(String, Vec<u8>, u32, u32), String> {
    let (mime, bytes) = parse_image_data(data_url)?;
    let (width, height) = check_image_header(&mime, &bytes)?;
    Ok((mime, bytes, width, height))
}

pub(crate) fn make_thumbnail(mime: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    check_image_header(mime, bytes)?;
    let image = image::load_from_memory(bytes).map_err(|e| invalid(e))?;
    let thumb = image.thumbnail(THUMB_SIZE, THUMB_SIZE);
    let mut out = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| db_failed("encode thumbnail", e))?;
    Ok(out.into_inner())
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    let _ = meta;
    false
}

/// File metadata that never follows links; linked entries report InvalidInput.
pub(crate) fn plain_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || is_reparse(&meta) {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "linked paths aren't allowed",
        ));
    }
    Ok(meta)
}

/// Requires a plain directory that can actually be read.
pub(crate) fn check_directory(path: &Path) -> Result<(), String> {
    let meta = plain_metadata(path).map_err(|e| io_failed("inspect directory", e))?;
    if !meta.is_dir() {
        return Err(io_failed(
            "inspect directory",
            format!("{} isn't a directory", path.display()),
        ));
    }
    fs::read_dir(path)
        .and_then(|mut entries| entries.next().transpose())
        .map_err(|e| io_failed("read directory", e))?;
    Ok(())
}

/// Bounded read of a plain regular file; never returns more than the file limit.
pub(crate) fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let meta = plain_metadata(path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, "not a file"));
    }
    let mut bytes = Vec::new();
    std::io::Read::take(fs::File::open(path)?, MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "file exceeds the size limit",
        ));
    }
    Ok(bytes)
}

pub(crate) fn write_original(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| invalid("asset path"))?;
    fs::create_dir_all(parent).map_err(|e| io_failed("create asset directory", e))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_failed("write asset", e))?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io_failed("write asset", e));
    }
    Ok(())
}

/// Writes an immutable snapshot. Existing identical content is kept; different content is refused.
pub(crate) fn write_snapshot(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(io_failed("write snapshot", e));
            }
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            let existing = read_bounded(path).map_err(|e| io_failed("read snapshot", e))?;
            if existing == bytes {
                Ok(false)
            } else {
                Err(invalid("saved input snapshot cannot be replaced"))
            }
        }
        Err(e) => Err(io_failed("write snapshot", e)),
    }
}

pub(crate) fn check_roots(store: &LibraryStore) -> Result<(), String> {
    for dir in [
        store.dir().to_path_buf(),
        store.input_root(),
        store.root.join("gallery"),
        store.images_root(),
    ] {
        check_directory(&dir)?;
    }
    Ok(())
}

pub(crate) fn is_genuine_missing(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::NotFound
}

pub(crate) fn remove_dir(path: &Path) -> Result<(), String> {
    match plain_metadata(path) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(io_failed("inspect asset directory", "not a directory"));
            }
        }
        Err(e) if is_genuine_missing(&e) => return Ok(()),
        Err(e) => return Err(io_failed("inspect asset directory", e)),
    }
    fs::remove_dir_all(path).map_err(|e| io_failed("remove asset directory", e))
}
