use crate::library::{
    check_roots, db_failed, decode_image_data, invalid, io_failed, is_genuine_missing,
    make_thumbnail, plain_metadata, read_bounded, remove_dir, sanitize_name, validate_ids,
    write_original, LibraryStore,
};
use crate::library_sql;
use crate::provider::GenerationDetails;
use crate::provider::ProviderError;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use rusqlite::OptionalExtension;
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_PAGE: u64 = 120;
const MAX_TAG_LEN: usize = 64;
const MAX_TAGS: usize = 64;
const MAX_NAME_LEN: usize = 240;
const MAX_BOARD_NAME: usize = 80;
const MAX_BOARDS: i64 = 100;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedImage {
    #[serde(rename = "dataUrl")]
    pub data_url: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "assetId", skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryItem {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub source: String,
    pub history_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub prompt: String,
    pub details: Option<GenerationDetails>,
    pub file_path: String,
    pub byte_size: u64,
    pub favorite: bool,
    pub board_ids: Vec<String>,
    pub tags: Vec<String>,
    pub availability: String,
    pub pending_delete: bool,
    pub thumbnail_revision: u64,
    pub has_thumbnail: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GalleryQuery {
    pub search: String,
    pub source: String,
    pub model: Option<String>,
    pub tag: Option<String>,
    pub created_from: Option<u64>,
    pub created_until: Option<u64>,
    pub favorite: Option<bool>,
    pub board_id: Option<String>,
    pub availability: String,
    pub sort: String,
    pub offset: u64,
    pub limit: u64,
}

impl Default for GalleryQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            source: "all".into(),
            model: None,
            tag: None,
            created_from: None,
            created_until: None,
            favorite: None,
            board_id: None,
            availability: "all".into(),
            sort: "newest".into(),
            offset: 0,
            limit: MAX_PAGE,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryPage {
    pub items: Vec<GalleryItem>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryFacets {
    pub models: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryPatch {
    pub name: Option<String>,
    pub favorite: Option<bool>,
    pub add_tags: Option<Vec<String>>,
    pub remove_tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub asset_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFailure {
    pub id: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryBatchResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<BatchFailure>,
    pub paths: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub total_count: u64,
    pub missing_count: u64,
    pub favorite_count: u64,
    pub pending_delete_count: u64,
    pub original_bytes: u64,
    pub thumbnail_bytes: u64,
    pub history_count: u64,
    pub last_checked_at: Option<u64>,
    pub database_bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceReport {
    pub checked: u64,
    pub missing: u64,
    pub restored: u64,
    pub rebuilt: u64,
    pub removed: u64,
    pub failed: Vec<BatchFailure>,
}

pub enum MaintainOp {
    Check,
    Rebuild,
    Cleanup,
}

fn gone() -> String {
    String::from(ProviderError::coded(
        "backend_gallery_gone",
        "Image was removed from the gallery",
    ))
}

fn missing_original() -> String {
    String::from(ProviderError::coded(
        "backend_gallery_missing",
        "The original image file is missing",
    ))
}

fn validate_mutation_ids(ids: &[String]) -> Result<(), String> {
    validate_ids(ids)?;
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    if ids.iter().any(|id| !seen.insert(id)) {
        return Err(invalid("duplicate image id"));
    }
    Ok(())
}

fn normalize_tag(tag: &str) -> Result<String, String> {
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > MAX_TAG_LEN {
        return Err(invalid(format!("invalid tag {tag:?}")));
    }
    Ok(tag)
}

fn normalize_board_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_BOARD_NAME {
        return Err(invalid("board name must be 1-80 characters"));
    }
    Ok(name)
}

fn board_missing() -> String {
    String::from(ProviderError::coded(
        "backend_board_gone",
        "The board was removed",
    ))
}

fn favorites_board_in(tx: &rusqlite::Transaction<'_>) -> Result<String, String> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM boards WHERE name = ?1 ORDER BY created_at, id LIMIT 1",
            [library_sql::FAVORITES_BOARD_NAME],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| db_failed("lookup board", e))?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let count: i64 = tx
        .query_row("SELECT COUNT(*) FROM boards", [], |r| r.get(0))
        .map_err(|e| db_failed("count boards", e))?;
    if count >= MAX_BOARDS {
        return Err(invalid("too many boards"));
    }
    let id = Uuid::new_v4().to_string();
    let sort: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM boards",
            [],
            |r| r.get(0),
        )
        .map_err(|e| db_failed("order boards", e))?;
    tx.execute(
        library_sql::BOARD_INSERT,
        params_from_iter([
            Value::Text(id.clone()),
            Value::Text(library_sql::FAVORITES_BOARD_NAME.to_string()),
            Value::Integer(now_ms() as i64),
            Value::Integer(sort),
        ]),
    )
    .map_err(|e| db_failed("create board", e))?;
    Ok(id)
}

fn load_board(conn: &rusqlite::Connection, id: &str) -> Result<Option<Board>, String> {
    conn.query_row(
        r#"
SELECT b.id, b.name, b.created_at,
    (SELECT COUNT(*) FROM board_assets a
     INNER JOIN assets s ON s.id = a.asset_id
     WHERE a.board_id = b.id AND s.pending_delete = 0) AS asset_count
FROM boards b WHERE b.id = ?1
"#,
        [id],
        |r| {
            Ok(Board {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get::<_, i64>(2)? as u64,
                asset_count: r.get::<_, i64>(3)? as u64,
            })
        },
    )
    .optional()
    .map_err(|e| db_failed("lookup board", e))
}

fn item_from_row(root: &Path, row: &Row) -> Result<GalleryItem, rusqlite::Error> {
    let id: String = row.get(0)?;
    let mime: String = row.get(5)?;
    let details_json: Option<String> = row.get(11)?;
    let tags_json: String = row.get(18)?;
    let board_ids_json: String = row.get(19)?;
    Ok(GalleryItem {
        file_path: root
            .join("gallery")
            .join("images")
            .join(&id)
            .join(format!("original.{}", crate::library::extension(&mime)))
            .to_string_lossy()
            .into_owned(),
        id,
        name: row.get(1)?,
        created_at: row.get::<_, i64>(2)? as u64,
        width: row.get::<_, i64>(3)? as u32,
        height: row.get::<_, i64>(4)? as u32,
        mime,
        source: row.get(6)?,
        history_id: row.get(7)?,
        model: row.get(8)?,
        provider: row.get(9)?,
        prompt: row.get(10)?,
        details: details_json.and_then(|d| serde_json::from_str(&d).ok()),
        byte_size: row.get::<_, i64>(12)? as u64,
        favorite: row.get::<_, i64>(13)? != 0,
        pending_delete: row.get::<_, i64>(14)? != 0,
        availability: row.get(15)?,
        thumbnail_revision: row.get::<_, i64>(16)? as u64,
        has_thumbnail: row.get::<_, i64>(17)? != 0,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        board_ids: serde_json::from_str(&board_ids_json).unwrap_or_default(),
    })
}

#[derive(Clone, Copy)]
enum BoardMembership {
    Add,
    Remove,
}

impl LibraryStore {
    pub(crate) fn find_asset(&self, id: &str) -> Result<Option<GalleryItem>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {} {}",
                    library_sql::ASSET_COLUMNS,
                    library_sql::ASSET_BY_ID
                ),
                [id],
                |r| item_from_row(&self.root, r),
            )
            .optional()
            .map_err(|e| db_failed("lookup image", e))
    }

    /// Inspects the original after confirming readable roots and the asset directory.
    /// Returns the actual size when the original is valid, None when it is confirmed
    /// missing, and an error for anything unreadable or inconsistent — without any
    /// DB state change on error.
    pub(crate) fn inspect(&self, item: &GalleryItem) -> Result<Option<u64>, String> {
        check_roots(self)?;
        let missing = |store: &LibraryStore, item: &GalleryItem| -> Result<Option<u64>, String> {
            if item.availability != "missing" {
                store
                    .conn
                    .execute(library_sql::MARK_MISSING, [&item.id])
                    .map_err(|e| db_failed("update image state", e))?;
            }
            Ok(None)
        };
        let dir_meta = match plain_metadata(&self.asset_dir(&item.id)) {
            Ok(meta) => meta,
            Err(e) if is_genuine_missing(&e) => return missing(self, item),
            Err(e) => return Err(io_failed("inspect image", e)),
        };
        if !dir_meta.is_dir() {
            return Err(io_failed("inspect image", "asset path isn't a directory"));
        }
        let path = Path::new(&item.file_path);
        let meta = match plain_metadata(path) {
            Ok(meta) => meta,
            Err(e) if is_genuine_missing(&e) => return missing(self, item),
            Err(e) => return Err(io_failed("inspect image", e)),
        };
        if !meta.is_file() {
            return Err(io_failed("inspect image", "original isn't a file"));
        }
        if meta.len() > 64 * 1024 * 1024 {
            return Err(invalid("image exceeds the size limit"));
        }
        let reader = match image::ImageReader::open(path) {
            Ok(reader) => reader,
            Err(e) if is_genuine_missing(&e) => return missing(self, item),
            Err(e) => return Err(io_failed("inspect image", e)),
        };
        let reader = reader.with_guessed_format().map_err(|e| invalid(e))?;
        let expected = reader
            .format()
            .map(|f| match f {
                image::ImageFormat::Png => "image/png",
                image::ImageFormat::Jpeg => "image/jpeg",
                image::ImageFormat::WebP => "image/webp",
                image::ImageFormat::Gif => "image/gif",
                _ => "",
            })
            .unwrap_or("");
        let (width, height) = reader.into_dimensions().map_err(|e| invalid(e))?;
        if expected != item.mime || width != item.width || height != item.height {
            return Err(invalid("image doesn't match its record"));
        }
        let size = meta.len();
        if item.availability != "available" || item.byte_size != size {
            self.conn
                .execute(
                    library_sql::MARK_AVAILABLE,
                    params_from_iter([Value::Text(item.id.clone()), Value::Integer(size as i64)]),
                )
                .map_err(|e| db_failed("update image state", e))?;
        }
        Ok(Some(size))
    }

    /// Reads a managed original with all bounds applied. A NotFound after a
    /// successful inspect re-checks the roots before marking the image missing.
    fn read_asset(&self, item: &GalleryItem) -> Result<Vec<u8>, String> {
        match read_bounded(Path::new(&item.file_path)) {
            Ok(bytes) => Ok(bytes),
            Err(e) if is_genuine_missing(&e) => {
                check_roots(self)?;
                self.conn
                    .execute(library_sql::MARK_MISSING, [&item.id])
                    .map_err(|e| db_failed("update image state", e))?;
                Err(missing_original())
            }
            Err(e) => Err(io_failed("read image", e)),
        }
    }

    fn search_text(name: &str, prompt: &str, model: Option<&str>) -> String {
        [name, prompt, model.unwrap_or("")]
            .join("\n")
            .to_lowercase()
    }

    pub fn gallery_query(&self, query: GalleryQuery) -> Result<GalleryPage, String> {
        if query.limit == 0 || query.limit > MAX_PAGE {
            return Err(invalid("page size must be 1-120"));
        }
        if !matches!(
            query.source.as_str(),
            "all" | "imported" | "generated" | "file" | "clipboard" | "url"
        ) {
            return Err(invalid("invalid source filter"));
        }
        if !matches!(
            query.availability.as_str(),
            "all" | "available" | "missing" | "pending"
        ) {
            return Err(invalid("invalid availability filter"));
        }
        let order = match query.sort.as_str() {
            "newest" => library_sql::ORDER_NEWEST,
            "oldest" => library_sql::ORDER_OLDEST,
            "name" => library_sql::ORDER_NAME,
            "size" => library_sql::ORDER_SIZE,
            _ => return Err(invalid("invalid sort")),
        };
        if let (Some(from), Some(until)) = (query.created_from, query.created_until) {
            if from >= until {
                return Err(invalid("inverted date range"));
            }
        }
        let tag = match &query.tag {
            Some(tag) => Some(normalize_tag(tag)?),
            None => None,
        };
        if let Some(board_id) = &query.board_id {
            crate::library::validate_id(board_id)?;
        }
        let filter: Vec<Value> = vec![
            Value::Text(query.search.trim().to_lowercase()),
            Value::Text(query.source.clone()),
            query.model.clone().map(Value::Text).unwrap_or(Value::Null),
            tag.map(Value::Text).unwrap_or(Value::Null),
            query
                .created_from
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Null),
            query
                .created_until
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Null),
            query
                .favorite
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Null),
            Value::Text(query.availability.clone()),
            query
                .board_id
                .clone()
                .map(Value::Text)
                .unwrap_or(Value::Null),
        ];
        let total: u64 = self
            .conn
            .query_row(
                &format!("SELECT COUNT(*) {}", library_sql::ASSET_FILTER),
                params_from_iter(filter.clone()),
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| db_failed("count images", e))? as u64;
        let mut page = filter;
        page.push(Value::Integer(query.limit as i64));
        page.push(Value::Integer(query.offset as i64));
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} {} {} {}",
                library_sql::ASSET_COLUMNS,
                library_sql::ASSET_FILTER,
                order,
                library_sql::PAGE
            ))
            .map_err(|e| db_failed("list images", e))?;
        let items = stmt
            .query_map(params_from_iter(page), |r| item_from_row(&self.root, r))
            .map_err(|e| db_failed("list images", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list images", e))?;
        Ok(GalleryPage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
        })
    }

    pub fn gallery_facets(&self) -> Result<GalleryFacets, String> {
        let mut tags = self
            .conn
            .prepare(library_sql::TAGS)
            .map_err(|e| db_failed("list tags", e))?;
        let tags = tags
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| db_failed("list tags", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list tags", e))?;
        let mut models = self
            .conn
            .prepare(library_sql::MODELS)
            .map_err(|e| db_failed("list models", e))?;
        let models = models
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| db_failed("list models", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list models", e))?;
        Ok(GalleryFacets { models, tags })
    }

    pub fn gallery_get(&self, ids: &[String]) -> Result<Vec<GalleryItem>, String> {
        validate_ids(ids)?;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(mut item) = self.find_asset(id)? else {
                continue;
            };
            if !item.pending_delete {
                match self.inspect(&item)? {
                    Some(size) => {
                        item.availability = "available".into();
                        item.byte_size = size;
                    }
                    None => item.availability = "missing".into(),
                }
            }
            items.push(item);
        }
        Ok(items)
    }

    pub fn gallery_import(
        &mut self,
        data_url: &str,
        name: &str,
        source: &str,
    ) -> Result<GalleryItem, String> {
        check_roots(self)?;
        if !matches!(source, "file" | "clipboard" | "url") {
            return Err(invalid("invalid import source"));
        }
        let name = name.trim();
        let name: String = if name.chars().count() > MAX_NAME_LEN {
            name.chars().take(MAX_NAME_LEN).collect()
        } else {
            name.into()
        };
        let (mime, bytes, width, height) = decode_image_data(data_url)?;
        let thumbnail = make_thumbnail(&mime, &bytes)?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.asset_path(&id, &mime);
        let parent = path.parent().ok_or_else(|| invalid("asset path"))?;
        fs::create_dir_all(parent).map_err(|e| io_failed("create asset directory", e))?;
        self.check_dir_chain(parent)?;
        write_original(&path, &bytes)?;
        let created = now_ms();
        let insert = (|| -> Result<(), String> {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| db_failed("import", e))?;
            tx.execute(
                library_sql::INSERT_ASSET,
                params_from_iter([
                    Value::Text(id.clone()),
                    Value::Text(name.to_string()),
                    Value::Integer(created as i64),
                    Value::Integer(width as i64),
                    Value::Integer(height as i64),
                    Value::Text(mime.clone()),
                    Value::Text(source.to_string()),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Text(String::new()),
                    Value::Null,
                    Value::Integer(bytes.len() as i64),
                    Value::Text(Self::search_text(&name, "", None)),
                    Value::Blob(thumbnail),
                ]),
            )
            .map_err(|e| db_failed("import image", e))?;
            tx.commit().map_err(|e| db_failed("import image", e))
        })();
        if insert.is_err() {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(self.asset_dir(&id));
        }
        insert?;
        self.find_asset(&id)?
            .ok_or_else(|| db_failed("import image", "missing row"))
    }

    pub fn gallery_read(&self, id: &str) -> Result<ImportedImage, String> {
        crate::library::validate_id(id)?;
        let item = self.find_asset(id)?.ok_or_else(gone)?;
        if item.pending_delete {
            return Err(gone());
        }
        if self.inspect(&item)?.is_none() {
            return Err(missing_original());
        }
        let bytes = self.read_asset(&item)?;
        use base64::Engine;
        Ok(ImportedImage {
            data_url: format!(
                "data:{};base64,{}",
                item.mime,
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ),
            name: if item.name.is_empty() {
                item.id.clone()
            } else {
                item.name
            },
            width: item.width,
            height: item.height,
            asset_id: Some(item.id),
        })
    }

    pub fn gallery_patch(
        &mut self,
        ids: &[String],
        patch: GalleryPatch,
    ) -> Result<Vec<GalleryItem>, String> {
        validate_mutation_ids(ids)?;
        if patch.name.is_none()
            && patch.favorite.is_none()
            && patch.add_tags.is_none()
            && patch.remove_tags.is_none()
        {
            return Err(invalid("empty patch"));
        }
        let name = patch
            .name
            .map(|n| {
                let n = n.trim().to_string();
                if n.is_empty() || n.chars().count() > MAX_NAME_LEN {
                    return Err(invalid("name must be 1-240 characters"));
                }
                Ok(n)
            })
            .transpose()?;
        if name.is_some() && ids.len() != 1 {
            return Err(invalid("name applies to a single image"));
        }
        let add: Vec<String> = patch
            .add_tags
            .unwrap_or_default()
            .iter()
            .map(|t| normalize_tag(t))
            .collect::<Result<_, _>>()?;
        let add: Vec<String> = add.into_iter().fold(Vec::new(), |mut acc, tag| {
            if !acc.contains(&tag) {
                acc.push(tag);
            }
            acc
        });
        let remove: Vec<String> = patch
            .remove_tags
            .unwrap_or_default()
            .iter()
            .map(|t| normalize_tag(t))
            .collect::<Result<_, _>>()?;
        if add.iter().any(|t| remove.contains(t)) {
            return Err(invalid("tag can't be added and removed at once"));
        }
        let root = self.root.clone();
        let by_id = format!(
            "SELECT {} {}",
            library_sql::ASSET_COLUMNS,
            library_sql::ASSET_BY_ID
        );
        let tx = self
            .conn
            .transaction()
            .map_err(|e| db_failed("update images", e))?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            let row = tx
                .query_row(&by_id, [id], |r| item_from_row(&root, r))
                .optional()
                .map_err(|e| db_failed("lookup image", e))?;
            match row {
                Some(item) if !item.pending_delete => {
                    let kept: Vec<&String> =
                        item.tags.iter().filter(|t| !remove.contains(t)).collect();
                    let added = add.iter().filter(|t| !kept.contains(t)).count();
                    if kept.len() + added > MAX_TAGS {
                        let _ = tx.rollback();
                        return Err(invalid("too many tags"));
                    }
                    rows.push(item);
                }
                _ => {
                    let _ = tx.rollback();
                    return Err(gone());
                }
            }
        }
        for item in &rows {
            if let Some(name) = &name {
                tx.execute(
                    library_sql::RENAME,
                    params_from_iter([
                        Value::Text(item.id.clone()),
                        Value::Text(name.clone()),
                        Value::Text(Self::search_text(name, &item.prompt, item.model.as_deref())),
                    ]),
                )
                .map_err(|e| db_failed("rename image", e))?;
            }
            if let Some(favorite) = patch.favorite {
                if favorite {
                    let board_id = favorites_board_in(&tx)?;
                    tx.execute(
                        library_sql::BOARD_ADD_ASSET,
                        params_from_iter([
                            Value::Text(board_id),
                            Value::Text(item.id.clone()),
                            Value::Integer(now_ms() as i64),
                        ]),
                    )
                    .map_err(|e| db_failed("add to board", e))?;
                } else {
                    tx.execute("DELETE FROM board_assets WHERE asset_id = ?1", [&item.id])
                        .map_err(|e| db_failed("leave boards", e))?;
                }
                tx.execute(library_sql::FAVORITE_FROM_MEMBERSHIP, [&item.id])
                    .map_err(|e| db_failed("update image", e))?;
            }
            for tag in &add {
                tx.execute(
                    library_sql::ADD_TAG,
                    params_from_iter([Value::Text(item.id.clone()), Value::Text(tag.clone())]),
                )
                .map_err(|e| db_failed("add tag", e))?;
            }
            for tag in &remove {
                tx.execute(
                    library_sql::REMOVE_TAG,
                    params_from_iter([Value::Text(item.id.clone()), Value::Text(tag.clone())]),
                )
                .map_err(|e| db_failed("remove tag", e))?;
            }
        }
        tx.commit().map_err(|e| db_failed("update images", e))?;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(item) = self.find_asset(id)? {
                items.push(item);
            }
        }
        Ok(items)
    }

    pub fn gallery_delete(&mut self, ids: &[String]) -> Result<GalleryBatchResult, String> {
        check_roots(self)?;
        validate_mutation_ids(ids)?;
        let mut result = GalleryBatchResult::default();
        for id in ids {
            let step = (|| -> Result<(), String> {
                let Some(item) = self.find_asset(id)? else {
                    return Err(gone());
                };
                if !item.pending_delete {
                    self.conn
                        .execute(library_sql::MARK_DELETE, [id])
                        .map_err(|e| db_failed("delete image", e))?;
                }
                remove_dir(&self.asset_dir(id))?;
                self.conn
                    .execute(library_sql::DELETE_ASSET, [id])
                    .map_err(|e| db_failed("delete image", e))?;
                Ok(())
            })();
            match step {
                Ok(()) => result.succeeded.push(id.clone()),
                Err(e) => result.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        Ok(result)
    }

    pub fn gallery_export(
        &self,
        ids: &[String],
        directory: &str,
    ) -> Result<GalleryBatchResult, String> {
        validate_mutation_ids(ids)?;
        let target = PathBuf::from(directory);
        match fs::metadata(&target) {
            Ok(meta) if meta.is_dir() => {}
            _ => return Err(io_failed("inspect export directory", "not a directory")),
        }
        let target = target
            .canonicalize()
            .map_err(|e| io_failed("inspect export directory", e))?;
        let root = self
            .root
            .canonicalize()
            .map_err(|e| io_failed("inspect library", e))?;
        if target == root || target.starts_with(&root) {
            return Err(invalid("can't export into the library"));
        }
        let mut result = GalleryBatchResult::default();
        for id in ids {
            let step = (|| -> Result<String, String> {
                let item = self.find_asset(id)?.ok_or_else(gone)?;
                if item.pending_delete {
                    return Err(gone());
                }
                if self.inspect(&item)?.is_none() {
                    return Err(missing_original());
                }
                let bytes = self.read_asset(&item)?;
                let base = if item.name.is_empty() {
                    item.id.clone()
                } else {
                    sanitize_name(&item.name).unwrap_or_else(|_| item.id.clone())
                };
                let ext = crate::library::extension(&item.mime);
                for n in 0..1000u32 {
                    let suffix = if n == 0 {
                        String::new()
                    } else {
                        format!(" ({})", n + 1)
                    };
                    let candidate = target.join(format!("{base}-{id}{suffix}.{ext}"));
                    let mut file = match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&candidate)
                    {
                        Ok(file) => file,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(io_failed("write export", e)),
                    };
                    match file.write_all(&bytes).and_then(|_| file.sync_all()) {
                        Ok(()) => return Ok(candidate.to_string_lossy().into_owned()),
                        Err(e) => {
                            drop(file);
                            let _ = fs::remove_file(&candidate);
                            return Err(io_failed("write export", e));
                        }
                    }
                }
                Err(io_failed("write export", "no free file name"))
            })();
            match step {
                Ok(path) => {
                    result.succeeded.push(id.clone());
                    result.paths.insert(id.clone(), path);
                }
                Err(e) => result.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        Ok(result)
    }

    pub fn board_list(&self) -> Result<Vec<Board>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT b.id, b.name, b.created_at,
    (SELECT COUNT(*) FROM board_assets a
     INNER JOIN assets s ON s.id = a.asset_id
     WHERE a.board_id = b.id AND s.pending_delete = 0) AS asset_count
FROM boards b
ORDER BY b.sort_order ASC, b.created_at ASC, b.id ASC
"#,
            )
            .map_err(|e| db_failed("list boards", e))?;
        let boards = stmt
            .query_map([], |r| {
                Ok(Board {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get::<_, i64>(2)? as u64,
                    asset_count: r.get::<_, i64>(3)? as u64,
                })
            })
            .map_err(|e| db_failed("list boards", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list boards", e))?;
        Ok(boards)
    }

    pub fn board_create(&mut self, name: &str) -> Result<Board, String> {
        let name = normalize_board_name(name)?;
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM boards", [], |r| r.get(0))
            .map_err(|e| db_failed("count boards", e))?;
        if count >= MAX_BOARDS {
            return Err(invalid("too many boards"));
        }
        let id = Uuid::new_v4().to_string();
        let sort: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM boards",
                [],
                |r| r.get(0),
            )
            .map_err(|e| db_failed("order boards", e))?;
        self.conn
            .execute(
                library_sql::BOARD_INSERT,
                params_from_iter([
                    Value::Text(id.clone()),
                    Value::Text(name),
                    Value::Integer(now_ms() as i64),
                    Value::Integer(sort),
                ]),
            )
            .map_err(|e| db_failed("create board", e))?;
        load_board(&self.conn, &id)?.ok_or_else(|| invalid("board missing after create"))
    }

    pub fn board_rename(&mut self, id: &str, name: &str) -> Result<Board, String> {
        crate::library::validate_id(id)?;
        let name = normalize_board_name(name)?;
        let changed = self
            .conn
            .execute(
                library_sql::BOARD_RENAME,
                params_from_iter([Value::Text(id.to_string()), Value::Text(name)]),
            )
            .map_err(|e| db_failed("rename board", e))?;
        if changed == 0 {
            return Err(board_missing());
        }
        load_board(&self.conn, id)?.ok_or_else(board_missing)
    }

    pub fn board_delete(&mut self, id: &str) -> Result<(), String> {
        crate::library::validate_id(id)?;
        let members: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT asset_id FROM board_assets WHERE board_id = ?1")
                .map_err(|e| db_failed("list board images", e))?;
            let rows = stmt
                .query_map([id], |r| r.get(0))
                .map_err(|e| db_failed("list board images", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| db_failed("list board images", e))?;
            rows
        };
        let tx = self
            .conn
            .transaction()
            .map_err(|e| db_failed("delete board", e))?;
        let changed = tx
            .execute(library_sql::BOARD_DELETE, [id])
            .map_err(|e| db_failed("delete board", e))?;
        if changed == 0 {
            let _ = tx.rollback();
            return Err(board_missing());
        }
        for asset_id in members {
            tx.execute(library_sql::FAVORITE_FROM_MEMBERSHIP, [&asset_id])
                .map_err(|e| db_failed("update image", e))?;
        }
        tx.commit().map_err(|e| db_failed("delete board", e))?;
        Ok(())
    }

    pub fn board_add(
        &mut self,
        board_id: &str,
        ids: &[String],
    ) -> Result<GalleryBatchResult, String> {
        self.mutate_board_membership(board_id, ids, BoardMembership::Add)
    }

    pub fn board_remove(
        &mut self,
        board_id: &str,
        ids: &[String],
    ) -> Result<GalleryBatchResult, String> {
        self.mutate_board_membership(board_id, ids, BoardMembership::Remove)
    }

    pub fn board_move(
        &mut self,
        from_board_id: &str,
        to_board_id: &str,
        ids: &[String],
    ) -> Result<GalleryBatchResult, String> {
        crate::library::validate_id(from_board_id)?;
        crate::library::validate_id(to_board_id)?;
        if from_board_id == to_board_id {
            return Err(invalid("move needs two different boards"));
        }
        validate_mutation_ids(ids)?;
        if load_board(&self.conn, from_board_id)?.is_none()
            || load_board(&self.conn, to_board_id)?.is_none()
        {
            return Err(board_missing());
        }
        let root = self.root.clone();
        let mut result = GalleryBatchResult::default();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| db_failed("move board images", e))?;
        for id in ids {
            let step = (|| -> Result<(), String> {
                let item = tx
                    .query_row(
                        &format!(
                            "SELECT {} {}",
                            library_sql::ASSET_COLUMNS,
                            library_sql::ASSET_BY_ID
                        ),
                        [id],
                        |r| item_from_row(&root, r),
                    )
                    .optional()
                    .map_err(|e| db_failed("lookup image", e))?;
                match item {
                    Some(item) if !item.pending_delete => {}
                    _ => return Err(gone()),
                };
                tx.execute(
                    library_sql::BOARD_ADD_ASSET,
                    params_from_iter([
                        Value::Text(to_board_id.to_string()),
                        Value::Text(id.clone()),
                        Value::Integer(now_ms() as i64),
                    ]),
                )
                .map_err(|e| db_failed("add to board", e))?;
                tx.execute(
                    library_sql::BOARD_REMOVE_ASSET,
                    params_from_iter([
                        Value::Text(from_board_id.to_string()),
                        Value::Text(id.clone()),
                    ]),
                )
                .map_err(|e| db_failed("leave board", e))?;
                tx.execute(library_sql::FAVORITE_FROM_MEMBERSHIP, [id])
                    .map_err(|e| db_failed("update image", e))?;
                Ok(())
            })();
            match step {
                Ok(()) => result.succeeded.push(id.clone()),
                Err(e) => result.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        tx.commit()
            .map_err(|e| db_failed("move board images", e))?;
        Ok(result)
    }

    fn mutate_board_membership(
        &mut self,
        board_id: &str,
        ids: &[String],
        op: BoardMembership,
    ) -> Result<GalleryBatchResult, String> {
        crate::library::validate_id(board_id)?;
        validate_mutation_ids(ids)?;
        if load_board(&self.conn, board_id)?.is_none() {
            return Err(board_missing());
        }
        let root = self.root.clone();
        let mut result = GalleryBatchResult::default();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| db_failed("update board", e))?;
        for id in ids {
            let step = (|| -> Result<(), String> {
                let item = tx
                    .query_row(
                        &format!(
                            "SELECT {} {}",
                            library_sql::ASSET_COLUMNS,
                            library_sql::ASSET_BY_ID
                        ),
                        [id],
                        |r| item_from_row(&root, r),
                    )
                    .optional()
                    .map_err(|e| db_failed("lookup image", e))?;
                match item {
                    Some(item) if !item.pending_delete => {}
                    _ => return Err(gone()),
                };
                match op {
                    BoardMembership::Add => {
                        tx.execute(
                            library_sql::BOARD_ADD_ASSET,
                            params_from_iter([
                                Value::Text(board_id.to_string()),
                                Value::Text(id.clone()),
                                Value::Integer(now_ms() as i64),
                            ]),
                        )
                        .map_err(|e| db_failed("add to board", e))?;
                    }
                    BoardMembership::Remove => {
                        tx.execute(
                            library_sql::BOARD_REMOVE_ASSET,
                            params_from_iter([
                                Value::Text(board_id.to_string()),
                                Value::Text(id.clone()),
                            ]),
                        )
                        .map_err(|e| db_failed("leave board", e))?;
                    }
                }
                tx.execute(library_sql::FAVORITE_FROM_MEMBERSHIP, [id])
                    .map_err(|e| db_failed("update image", e))?;
                Ok(())
            })();
            match step {
                Ok(()) => result.succeeded.push(id.clone()),
                Err(e) => result.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        tx.commit().map_err(|e| db_failed("update board", e))?;
        Ok(result)
    }

    pub fn thumbnail(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        crate::library::validate_id(id)?;
        self.conn
            .query_row(library_sql::THUMBNAIL, [id], |r| {
                r.get::<_, Option<Vec<u8>>>(0)
            })
            .optional()
            .map(|row| row.flatten())
            .map_err(|e| db_failed("read thumbnail", e))
    }

    pub fn library_stats(&self) -> Result<LibraryStats, String> {
        check_roots(self)?;
        let mut stmt = self
            .conn
            .prepare(library_sql::STATS)
            .map_err(|e| db_failed("measure library", e))?;
        let (
            total_count,
            missing_count,
            favorite_count,
            pending_delete_count,
            original_bytes,
            thumbnail_bytes,
            history_count,
            last_checked_at,
        ) = stmt
            .query_row([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|e| db_failed("measure library", e))?;
        let last_checked_at = last_checked_at.and_then(|v| v.parse::<u64>().ok());
        let mut database_bytes = 0u64;
        for suffix in ["", "-wal", "-shm"] {
            let file = self
                .root
                .join(format!("{}{}", crate::library::DB_FILE, suffix));
            match plain_metadata(&file) {
                Ok(meta) => database_bytes += meta.len(),
                Err(e) if is_genuine_missing(&e) => {
                    if suffix.is_empty() {
                        return Err(io_failed("measure database", e));
                    }
                }
                Err(e) => return Err(io_failed("measure database", e)),
            }
        }
        Ok(LibraryStats {
            total_count: total_count as u64,
            missing_count: missing_count as u64,
            favorite_count: favorite_count as u64,
            pending_delete_count: pending_delete_count as u64,
            original_bytes: original_bytes.unwrap_or(0) as u64,
            thumbnail_bytes: thumbnail_bytes.unwrap_or(0) as u64,
            history_count: history_count as u64,
            last_checked_at,
            database_bytes,
        })
    }

    pub fn retry_pending_deletes(&mut self) -> Result<(), String> {
        check_roots(self)?;
        let mut stmt = self
            .conn
            .prepare(library_sql::PENDING_IDS)
            .map_err(|e| db_failed("list deleted images", e))?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| db_failed("list deleted images", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list deleted images", e))?;
        for id in ids {
            if remove_dir(&self.asset_dir(&id)).is_ok() {
                let _ = self.conn.execute(library_sql::DELETE_ASSET, [&id]);
            }
        }
        Ok(())
    }

    pub fn maintain(&mut self, op: MaintainOp) -> Result<MaintenanceReport, String> {
        check_roots(self)?;
        match op {
            MaintainOp::Check => self.maintain_check(),
            MaintainOp::Rebuild => self.maintain_rebuild(),
            MaintainOp::Cleanup => self.maintain_cleanup(),
        }
    }

    fn asset_ids(&self, sql: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| db_failed("list images", e))?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| db_failed("list images", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_failed("list images", e))?;
        Ok(ids)
    }

    fn maintain_check(&mut self) -> Result<MaintenanceReport, String> {
        enum Outcome {
            Present { restored: bool },
            Missing,
            Skipped,
        }
        let ids = self.asset_ids(&format!("SELECT a.id {}", library_sql::ALL_ASSETS))?;
        let mut report = MaintenanceReport::default();
        for id in ids {
            let step = (|| -> Result<Outcome, String> {
                let item = self.find_asset(&id)?.ok_or_else(gone)?;
                if item.pending_delete {
                    return Ok(Outcome::Skipped);
                }
                let was_missing = item.availability != "available";
                match self.inspect(&item)? {
                    None => Ok(Outcome::Missing),
                    Some(_) => Ok(Outcome::Present {
                        restored: was_missing,
                    }),
                }
            })();
            match step {
                Ok(Outcome::Missing) => {
                    report.checked += 1;
                    report.missing += 1;
                }
                Ok(Outcome::Present { restored }) => {
                    report.checked += 1;
                    if restored {
                        report.restored += 1;
                    }
                }
                Ok(Outcome::Skipped) => {}
                Err(e) => report.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        if report.failed.is_empty() {
            self.conn
                .execute(library_sql::SET_CHECKED, [now_ms().to_string()])
                .map_err(|e| db_failed("record check", e))?;
        }
        Ok(report)
    }

    fn maintain_rebuild(&mut self) -> Result<MaintenanceReport, String> {
        enum Outcome {
            Rebuilt { restored: bool },
            Missing,
            Skipped,
        }
        let ids = self.asset_ids(&format!("SELECT a.id {}", library_sql::ALL_ASSETS))?;
        let mut report = MaintenanceReport::default();
        for id in ids {
            let step = (|| -> Result<Outcome, String> {
                let item = self.find_asset(&id)?.ok_or_else(gone)?;
                if item.pending_delete {
                    return Ok(Outcome::Skipped);
                }
                let was_missing = item.availability != "available";
                match self.inspect(&item)? {
                    None => Ok(Outcome::Missing),
                    Some(_) => {
                        let bytes = self.read_asset(&item)?;
                        let thumbnail = make_thumbnail(&item.mime, &bytes)?;
                        self.conn
                            .execute(
                                library_sql::SET_THUMBNAIL,
                                params_from_iter([Value::Text(id.clone()), Value::Blob(thumbnail)]),
                            )
                            .map_err(|e| db_failed("store thumbnail", e))?;
                        Ok(Outcome::Rebuilt {
                            restored: was_missing,
                        })
                    }
                }
            })();
            match step {
                Ok(Outcome::Missing) => {
                    report.checked += 1;
                    report.missing += 1;
                }
                Ok(Outcome::Rebuilt { restored }) => {
                    report.checked += 1;
                    report.rebuilt += 1;
                    if restored {
                        report.restored += 1;
                    }
                }
                Ok(Outcome::Skipped) => {}
                Err(e) => report.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        Ok(report)
    }

    fn maintain_cleanup(&mut self) -> Result<MaintenanceReport, String> {
        let ids = self.asset_ids(library_sql::MISSING_IDS)?;
        let mut report = MaintenanceReport::default();
        for id in ids {
            let step = (|| -> Result<bool, String> {
                let item = self.find_asset(&id)?.ok_or_else(gone)?;
                match self.inspect(&item)? {
                    Some(_) => Ok(false),
                    None => {
                        self.conn
                            .execute(library_sql::MARK_DELETE, [&id])
                            .map_err(|e| db_failed("delete image", e))?;
                        remove_dir(&self.asset_dir(&id))?;
                        self.conn
                            .execute(library_sql::DELETE_ASSET, [&id])
                            .map_err(|e| db_failed("delete image", e))?;
                        Ok(true)
                    }
                }
            })();
            match step {
                Ok(true) => report.removed += 1,
                Ok(false) => report.restored += 1,
                Err(e) => report.failed.push(BatchFailure {
                    id: id.clone(),
                    message: e,
                }),
            }
        }
        Ok(report)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine;

    pub fn png() -> String {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(32, 16)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
        )
    }

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("lutriui-gallery-{}", uuid::Uuid::new_v4()))
    }

    fn query_all() -> GalleryQuery {
        GalleryQuery::default()
    }

    #[test]
    fn imports_persist_owned_files_and_deletion_removes_both_images() {
        let dir = temp();
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let image = store
            .gallery_import(&png(), "../original", "clipboard")
            .unwrap();
        assert_eq!((image.width, image.height), (32, 16));
        assert!(PathBuf::from(&image.file_path).is_file());
        assert!(store.thumbnail(&image.id).unwrap().is_some());
        drop(store);
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        assert_eq!(store.gallery_query(query_all()).unwrap().total, 1);
        assert_eq!(
            store.find_asset(&image.id).unwrap().unwrap().name,
            "../original"
        );
        let result = store.gallery_delete(&[image.id.clone()]).unwrap();
        assert_eq!(result.succeeded, vec![image.id.clone()]);
        assert!(store.gallery_query(query_all()).unwrap().items.is_empty());
        assert!(!PathBuf::from(&image.file_path).exists());
        assert!(store.find_asset(&image.id).unwrap().is_none());
        assert!(store.gallery_delete(&["../outside".into()]).is_err());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn generation_save_is_idempotent_even_after_deletion_and_restart() {
        let dir = temp();
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let item = || -> crate::history::HistoryItem {
            serde_json::from_value(serde_json::json!({
                "id":"task","createdAt":1,"provider":"google","model":"gemini-nano-banana-2.1",
                "mode":"t2i","prompt":"test","finalPrompt":"test","params":{},"boxes":[],"status":"ok"
            }))
            .unwrap()
        };
        let files = || {
            vec![crate::history::GenerationFile {
                kind: "result".into(),
                name: "result_0".into(),
                data: png(),
            }]
        };
        let saved = store.history_save(item(), files()).unwrap();
        let id = saved.result_asset_ids[0].clone();
        let saved = store.history_save(item(), files()).unwrap();
        assert_eq!(saved.result_asset_ids[0], id);
        assert_eq!(store.gallery_query(query_all()).unwrap().total, 1);
        store.gallery_delete(&[id.clone()]).unwrap();
        drop(store);
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let saved = store.history_save(item(), files()).unwrap();
        assert_eq!(saved.result_asset_ids[0], id);
        assert_eq!(saved.result_files[0], "");
        assert_eq!(store.gallery_query(query_all()).unwrap().total, 0);
        let other = item();
        let mut second = vec![crate::history::GenerationFile {
            kind: "result".into(),
            name: "result_1".into(),
            data: png(),
        }];
        second.extend(files());
        let saved = store.history_save(other.clone(), second).unwrap();
        assert_eq!(saved.result_asset_ids.len(), 2);
        assert_eq!(store.gallery_query(query_all()).unwrap().total, 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interrupted_deletion_finishes_on_restart_and_corrupt_database_is_preserved() {
        let dir = temp();
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let asset = store.gallery_import(&png(), "file", "file").unwrap();
        store
            .conn
            .execute(library_sql::MARK_DELETE, [&asset.id])
            .unwrap();
        drop(store);
        let store = LibraryStore::new(dir.clone()).unwrap();
        assert_eq!(store.gallery_query(query_all()).unwrap().total, 0);
        assert!(!PathBuf::from(&asset.file_path).exists());
        drop(store);
        let garbage = vec![b'x'; 1024];
        std::fs::write(dir.join(crate::library::DB_FILE), &garbage).unwrap();
        assert!(LibraryStore::new(dir.clone()).is_err());
        assert_eq!(
            std::fs::read(dir.join(crate::library::DB_FILE)).unwrap(),
            garbage
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn star(favorite: bool) -> GalleryPatch {
        GalleryPatch {
            name: None,
            favorite: Some(favorite),
            add_tags: None,
            remove_tags: None,
        }
    }

    #[test]
    fn boards_hold_assets_and_replace_favorites() {
        let dir = temp();
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let a = store.gallery_import(&png(), "a", "file").unwrap();
        let b = store.gallery_import(&png(), "b", "file").unwrap();
        assert!(store.board_list().unwrap().is_empty());
        let board = store.board_create("Looks").unwrap();
        store.board_add(&board.id, &[a.id.clone()]).unwrap();
        let listed = store.board_list().unwrap();
        assert_eq!(listed[0].name, "Looks");
        assert_eq!(listed[0].asset_count, 1);
        let page = store
            .gallery_query(GalleryQuery {
                board_id: Some(board.id.clone()),
                ..query_all()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, a.id);
        assert!(page.items[0].favorite);
        assert_eq!(page.items[0].board_ids, vec![board.id.clone()]);
        store
            .board_add(&board.id, &[b.id.clone()])
            .unwrap();
        let other = store.board_create("Keep").unwrap();
        store
            .board_move(&board.id, &other.id, &[a.id.clone()])
            .unwrap();
        let looks = store
            .gallery_query(GalleryQuery {
                board_id: Some(board.id.clone()),
                ..query_all()
            })
            .unwrap();
        assert_eq!(looks.total, 1);
        assert_eq!(looks.items[0].id, b.id);
        store.board_remove(&other.id, &[a.id.clone()]).unwrap();
        let a = store.find_asset(&a.id).unwrap().unwrap();
        assert!(!a.favorite);
        assert!(a.board_ids.is_empty());
        store.board_rename(&other.id, "Kept").unwrap();
        store.board_delete(&board.id).unwrap();
        assert_eq!(store.board_list().unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_starred_assets_into_a_favorites_board() {
        let dir = temp();
        let mut store = LibraryStore::new(dir.clone()).unwrap();
        let kept = store.gallery_import(&png(), "kept", "file").unwrap();
        let _plain = store.gallery_import(&png(), "plain", "file").unwrap();
        store.gallery_patch(&[kept.id.clone()], star(true)).unwrap();
        store
            .conn
            .execute_batch("DROP TABLE board_assets; DROP TABLE boards; PRAGMA user_version = 1;")
            .unwrap();
        drop(store);
        let store = LibraryStore::new(dir.clone()).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let boards = store.board_list().unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].name, "收藏");
        assert_eq!(boards[0].asset_count, 1);
        let page = store
            .gallery_query(GalleryQuery {
                board_id: Some(boards[0].id.clone()),
                ..query_all()
            })
            .unwrap();
        assert_eq!(page.items[0].id, kept.id);
        let starred = store
            .gallery_query(GalleryQuery {
                favorite: Some(true),
                ..query_all()
            })
            .unwrap();
        assert_eq!(starred.total, 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v1_libraries_without_stars_gain_empty_boards() {
        let dir = temp();
        let store = LibraryStore::new(dir.clone()).unwrap();
        store
            .conn
            .execute_batch("DROP TABLE board_assets; DROP TABLE boards; PRAGMA user_version = 1;")
            .unwrap();
        drop(store);
        let store = LibraryStore::new(dir.clone()).unwrap();
        assert!(store.board_list().unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
