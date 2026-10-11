import { useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  Images,
  Plus,
  Clipboard,
  Trash,
  ArrowLeft,
  Eye,
  Copy,
  FolderSimple,
  PencilSimple,
  Export,
  FunnelSimple,
} from "@phosphor-icons/react";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  Board,
  GalleryAvailabilityFilter,
  GalleryBatchResult,
  GalleryItem,
  GalleryPatch,
  GalleryQuery,
  GallerySort,
  GallerySourceFilter,
} from "../lib/types";
import * as api from "../lib/api";
import { useGallery, GALLERY_PAGE_SIZE } from "../app/useGallery";
import { m, formatBytes, formatDate, formatDateTime } from "../i18n";
import { modelLabel } from "../labels";
import { modelByAnyId } from "../models/catalog";
import { exportDefaultPath } from "../lib/export";
import Modal from "./Modal";
import GenerationInfo from "./GenerationInfo";

function localDayStart(value: string): number | null {
  return value ? new Date(value + "T00:00:00").getTime() : null;
}

/** Upper bound is the next local calendar day, correct across DST changes. */
function localDayEnd(value: string): number | null {
  if (!value) return null;
  const day = new Date(value + "T00:00:00");
  day.setDate(day.getDate() + 1);
  return day.getTime();
}

function describeBatch(result: GalleryBatchResult, done: string) {
  if (!result.failed.length) return done;
  return (
    m.gallery_batch_failed({ count: result.failed.length }) +
    "：" +
    result.failed.map((f) => f.message).join("；")
  );
}

function batchSummary(result: GalleryBatchResult) {
  return m.gallery_batch_summary({
    succeeded: result.succeeded.length,
    failed: result.failed.length,
  });
}

function parseTags(input: string) {
  return input
    .split(/[,，]/)
    .map((t) => t.trim())
    .filter(Boolean);
}

export default function Gallery(p: {
  revision: number;
  importing: boolean;
  saveDirectory: string;
  onRefresh: () => Promise<void>;
  onFiles: () => void;
  onPaste: () => void;
  onUse: (ids: string[]) => Promise<void>;
  onEdit: (item: GalleryItem) => Promise<void | boolean>;
  picker?: { limit: number; usedIds: string[] };
}) {
  const [search, setSearch] = useState("");
  const [source, setSource] = useState<GallerySourceFilter>("all");
  const [model, setModel] = useState<string | null>(null);
  const [tag, setTag] = useState<string | null>(null);
  const [boardId, setBoardId] = useState<string | null>(null);
  const [boards, setBoards] = useState<Board[]>([]);
  const [creatingBoard, setCreatingBoard] = useState(false);
  const [boardName, setBoardName] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [deleteBoard, setDeleteBoard] = useState(false);
  const [availability, setAvailability] =
    useState<GalleryAvailabilityFilter>("all");
  const [fromDay, setFromDay] = useState("");
  const [toDay, setToDay] = useState("");
  const [sort, setSort] = useState<GallerySort>("newest");
  const [showFilters, setShowFilters] = useState(false);
  const [selection, setSelection] = useState<string[]>([]);
  const [selecting, setSelecting] = useState(!!p.picker);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [deleteIds, setDeleteIds] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [tagDraft, setTagDraft] = useState("");
  const [broken, setBroken] = useState<Set<string>>(new Set());
  useEffect(() => setBroken(new Set()), [p.revision]);
  const used = new Set(p.picker?.usedIds ?? []);
  const limit = p.picker?.limit ?? 500;

  const query = useMemo<GalleryQuery>(
    () => ({
      search: search.trim(),
      source,
      model,
      tag,
      createdFrom: localDayStart(fromDay),
      createdUntil: localDayEnd(toDay),
      favorite: null,
      boardId,
      availability,
      sort,
      offset: 0,
      limit: GALLERY_PAGE_SIZE,
    }),
    [
      search,
      source,
      model,
      tag,
      boardId,
      availability,
      fromDay,
      toDay,
      sort,
    ],
  );
  const data = useGallery(query, p.revision);
  const hasFilters =
    !!search.trim() ||
    source !== "all" ||
    model != null ||
    tag != null ||
    boardId != null ||
    availability !== "all" ||
    !!fromDay ||
    !!toDay;
  const clearSelection = () => setSelection([]);
  const filterChange = (apply: () => void) => {
    apply();
    clearSelection();
    setError("");
    setNotice("");
  };
  const groups = useMemo(() => {
    const map = new Map<string, GalleryItem[]>();
    if (sort === "name" || sort === "size") {
      map.set("", data.items);
      return map;
    }
    for (const item of data.items) {
      const date = formatDate(item.createdAt);
      map.set(date, [...(map.get(date) ?? []), item]);
    }
    return map;
  }, [data.items, sort]);
  const toggle = (id: string) => {
    setError("");
    setSelection((ids) =>
      ids.includes(id)
        ? ids.filter((v) => v !== id)
        : ids.length < limit
          ? [...ids, id]
          : ids,
    );
  };
  const run = async (action: () => Promise<void>) => {
    if (busy || p.importing) return;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    const result = await api.galleryDelete(deleteIds);
    await p.onRefresh();
    setSelection((ids) => ids.filter((id) => result.failed.some((f) => f.id === id)));
    setNotice(batchSummary(result));
    if (
      previewId &&
      deleteIds.includes(previewId) &&
      !result.failed.some((f) => f.id === previewId)
    )
      setPreviewId(null);
    setDeleteIds(result.failed.length ? result.failed.map((f) => f.id) : []);
    if (result.failed.length)
      throw new Error(describeBatch(result, ""));
  };
  const batchTags = async (add: boolean) => {
    const tags = parseTags(tagDraft);
    if (!tags.length || !selection.length) return;
    await api.galleryPatch(
      selection,
      add ? { addTags: tags } : { removeTags: tags },
    );
    setTagDraft("");
    await p.onRefresh();
  };
  const batchExport = async () => {
    if (!selection.length) return;
    const directory = await open({ directory: true, multiple: false });
    if (typeof directory !== "string") return;
    const result = await api.galleryExport(selection, directory);
    await p.onRefresh();
    setSelection((ids) =>
      ids.filter((id) => result.failed.some((f) => f.id === id)),
    );
    setNotice(batchSummary(result));
    if (result.failed.length) throw new Error(describeBatch(result, ""));
  };
  const reloadBoards = async () => {
    setBoards(await api.boardList());
  };
  useEffect(() => {
    void reloadBoards().catch((e) => setError(String(e)));
  }, [p.revision]);
  const activeBoard = boards.find((board) => board.id === boardId) ?? null;
  const finishBoard = async (action: () => Promise<unknown>) => {
    await action();
    await reloadBoards();
    await p.onRefresh();
  };
  const createBoard = async () => {
    const name = boardName.trim();
    if (!name) return;
    const board = await api.boardCreate(name);
    setBoardName("");
    setCreatingBoard(false);
    setBoardId(board.id);
    await reloadBoards();
  };
  const renameBoard = async () => {
    if (!activeBoard) return;
    const name = boardName.trim();
    if (!name || name === activeBoard.name) {
      setRenaming(false);
      return;
    }
    await api.boardRename(activeBoard.id, name);
    setRenaming(false);
    await reloadBoards();
  };
  const removeBoard = async () => {
    if (!activeBoard) return;
    await api.boardDelete(activeBoard.id);
    setDeleteBoard(false);
    setBoardId(null);
    await reloadBoards();
    await p.onRefresh();
  };
  const toggleBoard = async (item: GalleryItem, target: string) => {
    const on = item.boardIds.includes(target);
    await (on
      ? api.boardRemove(target, [item.id])
      : api.boardAdd(target, [item.id]));
    await reloadBoards();
    await p.onRefresh();
  };
  const selectedItems = data.items.filter((item) =>
    selection.includes(item.id),
  );
  const hasBlocked = selectedItems.some(
    (item) => item.pendingDelete || item.availability === "missing",
  );
  const hasPending = selectedItems.some((item) => item.pendingDelete);
  const selectableLoaded = data.items.filter(
    (item) =>
      !p.picker ||
      (!used.has(item.id) &&
        !item.pendingDelete &&
        item.availability === "available"),
  );
  const locked = busy || p.importing;
  return (
    <div className={"gallery" + (p.picker ? " gallery-picker" : "")}>
      <div className="gallery-toolbar">
        <div className="gallery-title">
          <Images size={23} />
          <h2>{p.picker ? m.gallery_pick() : m.gallery()}</h2>
          <span className="muted">
            {m.count_images({ count: data.total })}
          </span>
        </div>
        <div className="row">
          <button disabled={locked} onClick={p.onFiles}>
            <Plus size={16} />
            {m.gallery_import()}
          </button>
          <button disabled={locked} onClick={p.onPaste}>
            <Clipboard size={16} />
            {m.gallery_paste()}
          </button>
          {!p.picker && (
            <button
              disabled={locked || !data.total}
              aria-pressed={selecting}
              onClick={() => {
                setSelecting(!selecting);
                clearSelection();
                setError("");
              }}
            >
              {selecting ? m.gallery_done() : m.gallery_select()}
            </button>
          )}
        </div>
      </div>
      <div className="gallery-filters">
        <div className="segmented" aria-label={m.gallery_source()}>
          {(
            [
              ["all", m.gallery_all()],
              ["generated", m.gallery_generated()],
              ["imported", m.gallery_imported()],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              aria-label={m.gallery_filter({ label })}
              aria-pressed={source === value}
              className={source === value ? "active" : ""}
              onClick={() => filterChange(() => setSource(value))}
            >
              {label}
            </button>
          ))}
        </div>
        <input
          aria-label={m.gallery_search()}
          type="search"
          value={search}
          onChange={(e) => filterChange(() => setSearch(e.target.value))}
          placeholder={m.gallery_search_placeholder()}
        />
        <label className="gallery-sort">
          <span className="muted">{m.gallery_sort()}</span>
          <select
            aria-label={m.gallery_sort()}
            value={sort}
            onChange={(e) =>
              filterChange(() => setSort(e.target.value as GallerySort))
            }
          >
            <option value="newest">{m.gallery_sort_newest()}</option>
            <option value="oldest">{m.gallery_sort_oldest()}</option>
            <option value="name">{m.gallery_sort_name()}</option>
            <option value="size">{m.gallery_sort_size()}</option>
          </select>
        </label>
        <button
          aria-pressed={showFilters}
          aria-label={m.gallery_more_filters()}
          onClick={() => setShowFilters(!showFilters)}
        >
          <FunnelSimple size={16} />
        </button>
      </div>
      {showFilters && (
        <div className="gallery-filters gallery-filter-row">
          <label>
            <span className="muted">{m.gallery_model()}</span>
            <select
              aria-label={m.gallery_model()}
              value={model ?? ""}
              onChange={(e) =>
                filterChange(() => setModel(e.target.value || null))
              }
            >
              <option value="">{m.gallery_all()}</option>
              {data.facets.models.map((id) => (
                <option key={id} value={id}>
                  {modelLabel(id, modelByAnyId(id)?.label ?? id)}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="muted">{m.gallery_tag()}</span>
            <select
              aria-label={m.gallery_tag()}
              value={tag ?? ""}
              onChange={(e) =>
                filterChange(() => setTag(e.target.value || null))
              }
            >
              <option value="">{m.gallery_all()}</option>
              {data.facets.tags.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="muted">{m.gallery_availability()}</span>
            <select
              aria-label={m.gallery_availability()}
              value={availability}
              onChange={(e) =>
                filterChange(() =>
                  setAvailability(
                    e.target.value as GalleryAvailabilityFilter,
                  )
                )
              }
            >
              <option value="all">{m.gallery_all()}</option>
              <option value="available">{m.gallery_available()}</option>
              <option value="missing">{m.gallery_missing()}</option>
              <option value="pending">{m.gallery_pending()}</option>
            </select>
          </label>
          <label>
            <span className="muted">{m.gallery_from()}</span>
            <input
              type="date"
              aria-label={m.gallery_from()}
              value={fromDay}
              onChange={(e) => filterChange(() => setFromDay(e.target.value))}
            />
          </label>
          <label>
            <span className="muted">{m.gallery_to()}</span>
            <input
              type="date"
              aria-label={m.gallery_to()}
              value={toDay}
              onChange={(e) => filterChange(() => setToDay(e.target.value))}
            />
          </label>
        </div>
      )}
      <div className="gallery-boards" aria-label={m.board_bar()}>
        <button
          aria-pressed={boardId == null}
          className={boardId == null ? "active" : ""}
          onClick={() => filterChange(() => setBoardId(null))}
        >
          {m.board_all()}
        </button>
        {boards.map((board) => (
          <button
            key={board.id}
            aria-pressed={boardId === board.id}
            className={boardId === board.id ? "active" : ""}
            onClick={() => filterChange(() => setBoardId(board.id))}
          >
            {board.name}
            <span className="muted">{board.assetCount}</span>
          </button>
        ))}
        {creatingBoard ? (
          <span className="gallery-board-edit">
            <input
              autoFocus
              aria-label={m.board_name()}
              value={boardName}
              onChange={(e) => setBoardName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void run(createBoard);
                if (e.key === "Escape") {
                  setCreatingBoard(false);
                  setBoardName("");
                }
              }}
            />
            <button
              className="primary"
              disabled={locked || !boardName.trim()}
              onClick={() => void run(createBoard)}
            >
              {m.board_create()}
            </button>
          </span>
        ) : (
          <button
            disabled={locked}
            onClick={() => {
              setCreatingBoard(true);
              setRenaming(false);
              setBoardName("");
            }}
          >
            <Plus size={14} />
            {m.board_new()}
          </button>
        )}
        {activeBoard && !creatingBoard && (
          renaming ? (
            <span className="gallery-board-edit">
              <input
                autoFocus
                aria-label={m.board_rename()}
                value={boardName}
                onChange={(e) => setBoardName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void run(renameBoard);
                  if (e.key === "Escape") setRenaming(false);
                }}
              />
              <button
                disabled={locked || !boardName.trim()}
                onClick={() => void run(renameBoard)}
              >
                {m.board_save_name()}
              </button>
            </span>
          ) : (
            <>
              <button
                disabled={locked}
                onClick={() => {
                  setRenaming(true);
                  setBoardName(activeBoard.name);
                }}
              >
                {m.board_rename()}
              </button>
              <button
                className="danger"
                disabled={locked}
                onClick={() => setDeleteBoard(true)}
              >
                {m.board_delete()}
              </button>
            </>
          )
        )}
      </div>
      {p.picker && (
        <p className="gallery-hint">
          {m.gallery_room({
            count: Math.max(0, limit - selection.length),
          })}
        </p>
      )}
      {(error || data.error) && (
        <div className="gallery-error" role="alert">
          {error || data.error}
          <button
            onClick={() => {
              setError("");
              setBroken(new Set());
              data.refresh();
            }}
          >
            {m.gallery_refresh()}
          </button>
        </div>
      )}
      {notice && (
        <p className="gallery-hint" role="status">
          {notice}
        </p>
      )}
      {p.importing && (
        <p className="gallery-hint" role="status">
          {m.gallery_saving()}
        </p>
      )}
      <div className="gallery-scroll">
        {data.loading ? (
          <div className="gallery-empty" role="status">
            <Images size={44} weight="thin" />
            <strong>{m.gallery_loading()}</strong>
          </div>
        ) : !data.items.length ? (
          <div className="gallery-empty">
            <Images size={44} weight="thin" />
            <strong>
              {hasFilters ? m.gallery_no_match() : m.gallery_empty_title()}
            </strong>
            <p>
              {hasFilters
                ? m.gallery_no_match_help()
                : m.gallery_empty_help()}
            </p>
          </div>
        ) : (
          [...groups].map(([date, items]) => (
            <section className="gallery-day" key={date || "all"}>
              {date && <h3>{date}</h3>}
              <div className="gallery-grid">
                {items.map((item) => (
                  <GalleryTile
                    key={item.id}
                    item={item}
                    checked={selection.includes(item.id)}
                    order={
                      p.picker ? selection.indexOf(item.id) + 1 : undefined
                    }
                    alreadyUsed={used.has(item.id)}
                    selecting={selecting}
                    disabled={
                      locked ||
                      (!!p.picker &&
                        (used.has(item.id) ||
                          item.pendingDelete ||
                          item.availability === "missing" ||
                          (!selection.includes(item.id) &&
                            selection.length >= limit)))
                    }
                    thumbBroken={broken.has(
                      item.id + ":" + item.thumbnailRevision,
                    )}
                    onThumbError={() =>
                      setBroken((s) =>
                        s.has(item.id + ":" + item.thumbnailRevision)
                          ? s
                          : new Set(s).add(
                              item.id + ":" + item.thumbnailRevision,
                            ),
                      )
                    }
                    boards={boards}
                    activeBoardId={boardId}
                    onToggleBoard={(target) =>
                      void run(() => toggleBoard(item, target))
                    }
                    onEdit={
                      p.picker || item.pendingDelete
                        ? undefined
                        : () => void run(async () => {
                            await p.onEdit(item);
                          })
                    }
                    onClick={() => {
                      if (item.pendingDelete) {
                        setError("");
                        setDeleteIds([item.id]);
                      } else if (selecting) toggle(item.id);
                      else setPreviewId(item.id);
                    }}
                    onPeek={
                      p.picker && !item.pendingDelete
                        ? () => setPreviewId(item.id)
                        : undefined
                    }
                  />
                ))}
              </div>
            </section>
          ))
        )}
        {!data.loading && data.hasMore && (
          <button
            className="gallery-more"
            disabled={data.loadingMore}
            onClick={data.loadMore}
          >
            {data.loadingMore
              ? m.gallery_loading()
              : m.gallery_load_more({ count: data.total - data.items.length })}
          </button>
        )}
      </div>
      {selecting && (
        <div
          className="gallery-selection"
          role="region"
          aria-label={m.gallery_selection_actions()}
        >
          <span>{m.gallery_selected({ count: selection.length })}</span>
          {!p.picker && (
            <span className="muted">
              {m.gallery_select_limit({ count: limit })}
            </span>
          )}
          <button
            disabled={locked || !selectableLoaded.length}
            onClick={() =>
              setSelection(
                selectableLoaded.slice(0, limit).map((item) => item.id),
              )
            }
          >
            {m.gallery_select_all()}
          </button>
          <button
            disabled={locked || !selection.length}
            onClick={clearSelection}
          >
            {m.gallery_clear()}
          </button>
          <button
            className="primary"
            disabled={locked || !selection.length || hasBlocked}
            onClick={() => void run(() => p.onUse(selection))}
          >
            {selection.length
              ? m.gallery_add_refs_count({ count: selection.length })
              : m.gallery_add_refs()}
          </button>
          {!p.picker && (
            <>
              <label className="gallery-board-select">
                <span className="muted">{m.board_add_to()}</span>
                <select
                  aria-label={m.board_add()}
                  disabled={locked || !selection.length || hasPending || !boards.length}
                  value=""
                  onChange={(e) => {
                    const target = e.target.value;
                    e.target.value = "";
                    if (!target) return;
                    void run(() =>
                      finishBoard(() => api.boardAdd(target, selection)),
                    );
                  }}
                >
                  <option value="">{m.board_add()}</option>
                  {boards.map((board) => (
                    <option key={board.id} value={board.id}>
                      {board.name}
                    </option>
                  ))}
                </select>
              </label>
              {activeBoard && (
                <>
                  <button
                    disabled={locked || !selection.length || hasPending}
                    onClick={() =>
                      void run(() =>
                        finishBoard(() =>
                          api.boardRemove(activeBoard.id, selection),
                        ),
                      )
                    }
                  >
                    {m.board_leave()}
                  </button>
                  <label className="gallery-board-select">
                    <span className="muted">{m.board_move()}</span>
                    <select
                      aria-label={m.board_move()}
                      disabled={
                        locked ||
                        !selection.length ||
                        hasPending ||
                        boards.length < 2
                      }
                      value=""
                      onChange={(e) => {
                        const target = e.target.value;
                        e.target.value = "";
                        if (!target) return;
                        void run(() =>
                          finishBoard(() =>
                            api.boardMove(activeBoard.id, target, selection),
                          ),
                        );
                      }}
                    >
                      <option value="">{m.board_move()}</option>
                      {boards
                        .filter((board) => board.id !== activeBoard.id)
                        .map((board) => (
                          <option key={board.id} value={board.id}>
                            {board.name}
                          </option>
                        ))}
                    </select>
                  </label>
                </>
              )}
              <input
                className="gallery-tag-input"
                aria-label={m.gallery_tag_edit()}
                placeholder={m.gallery_tag_placeholder()}
                value={tagDraft}
                disabled={locked || !selection.length || hasPending}
                onChange={(e) => setTagDraft(e.target.value)}
              />
              <button
                disabled={
                  locked || !selection.length || hasPending || !tagDraft.trim()
                }
                onClick={() => void run(() => batchTags(true))}
              >
                {m.gallery_tag_add()}
              </button>
              <button
                disabled={
                  locked || !selection.length || hasPending || !tagDraft.trim()
                }
                onClick={() => void run(() => batchTags(false))}
              >
                {m.gallery_tag_remove()}
              </button>
              <button
                disabled={locked || !selection.length || hasBlocked}
                onClick={() => void run(batchExport)}
              >
                <Export size={16} />
                {m.gallery_export()}
              </button>
              <button
                className="danger"
                disabled={locked || !selection.length}
                onClick={() => {
                  setError("");
                  setDeleteIds(selection);
                }}
              >
                <Trash size={16} />
                {m.action_delete()}
              </button>
            </>
          )}
        </div>
      )}
      {previewId && (
        <GalleryDetail
          id={previewId}
          locked={locked}
          picker={p.picker}
          selected={selection.includes(previewId)}
          pickerFull={selection.length >= limit}
          alreadyUsed={used.has(previewId)}
          saveDirectory={p.saveDirectory}
          boards={boards}
          onToggleBoard={(item, target) => toggleBoard(item, target)}
          onClose={() => {
            if (!busy) {
              setPreviewId(null);
              setError("");
            }
          }}
          onChanged={p.onRefresh}
          onUse={p.onUse}
          onEdit={p.onEdit}
          onToggle={() => {
            toggle(previewId);
            setPreviewId(null);
          }}
          onDelete={(id) => {
            setError("");
            setDeleteIds([id]);
          }}
          error={error}
          setError={setError}
          run={run}
        />
      )}
      {deleteBoard && activeBoard && (
        <Modal
          title={m.board_delete_title()}
          onClose={() => {
            if (!busy) setDeleteBoard(false);
          }}
        >
          <p>{m.board_delete_body()}</p>
          {error && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="row">
            <button disabled={busy} onClick={() => setDeleteBoard(false)}>
              {m.action_cancel()}
            </button>
            <button
              className="danger"
              disabled={busy}
              onClick={() => void run(removeBoard)}
            >
              {m.board_delete()}
            </button>
          </div>
        </Modal>
      )}
      {!!deleteIds.length && (
        <Modal
          title={m.gallery_delete_title()}
          onClose={() => {
            if (!busy) {
              setDeleteIds([]);
              setError("");
            }
          }}
        >
          <p>{m.gallery_delete_body({ count: deleteIds.length })}</p>
          {error && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="row">
            <button
              disabled={busy}
              onClick={() => {
                setDeleteIds([]);
                setError("");
              }}
            >
              {m.action_cancel()}
            </button>
            <button
              className="danger"
              disabled={busy}
              onClick={() => void run(remove)}
            >
              {busy ? m.gallery_deleting() : m.gallery_confirm_delete()}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function GalleryTile(p: {
  item: GalleryItem;
  checked: boolean;
  order?: number;
  alreadyUsed: boolean;
  selecting: boolean;
  disabled: boolean;
  thumbBroken: boolean;
  boards: Board[];
  activeBoardId: string | null;
  onToggleBoard: (boardId: string) => void;
  onEdit?: () => void;
  onThumbError: () => void;
  onClick: () => void;
  onPeek?: () => void;
}) {
  const { item } = p;
  return (
    <div className="gallery-cell">
      <div className="gallery-tile-wrap">
      <button
        className={"gallery-tile" + (p.checked ? " selected" : "")}
        aria-label={
          p.selecting
            ? m.gallery_select_named({ name: item.name })
            : m.gallery_view_named({ name: item.name })
        }
        aria-pressed={p.selecting ? p.checked : undefined}
        disabled={p.disabled}
        title={item.name + " · " + item.width + "×" + item.height}
        onClick={p.onClick}
      >
        {!p.thumbBroken && item.hasThumbnail ? (
          <img
            src={api.galleryThumbnail(item.id, item.thumbnailRevision)}
            alt=""
            loading="lazy"
            decoding="async"
            onError={p.onThumbError}
          />
        ) : (
          <span className="gallery-thumb-empty" aria-hidden="true">
            <Images size={28} weight="thin" />
          </span>
        )}
        {item.pendingDelete ? (
          <span className="gallery-tile-label">
            {m.gallery_delete_incomplete()}
          </span>
        ) : item.availability === "missing" ? (
          <span className="gallery-tile-label gallery-missing">
            {m.gallery_missing()}
          </span>
        ) : p.alreadyUsed ? (
          <span className="gallery-tile-label">{m.gallery_in_task()}</span>
        ) : (
          p.selecting && (
            <span className={"gallery-check" + (p.checked ? " checked" : "")}>
              {p.checked &&
                (p.order != null ? (
                  p.order
                ) : (
                  <Check size={14} weight="bold" />
                ))}
            </span>
          )
        )}
      </button>
      {p.boards.length > 0 && (
        <BoardMark
          item={item}
          boards={p.boards}
          activeBoardId={p.activeBoardId}
          disabled={p.disabled || item.pendingDelete}
          onToggle={p.onToggleBoard}
        />
      )}
      {p.onEdit && (
        <button
          className="gallery-edit"
          aria-label={m.grid_edit_named()}
          disabled={
            p.disabled ||
            item.pendingDelete ||
            item.availability === "missing"
          }
          onClick={(e) => {
            e.stopPropagation();
            p.onEdit?.();
          }}
        >
          <PencilSimple size={14} />
          {m.grid_edit()}
        </button>
      )}
      {p.onPeek && (
        <button
          className="gallery-peek"
          aria-label={m.gallery_preview_named({ name: item.name })}
          onClick={p.onPeek}
        >
          <Eye size={16} />
        </button>
      )}
      </div>
      <span className="gallery-name" title={item.name}>
        {item.name}
      </span>
    </div>
  );
}

function BoardMark(p: {
  item: GalleryItem;
  boards: Board[];
  activeBoardId: string | null;
  disabled: boolean;
  onToggle: (boardId: string) => void;
}) {
  const onActive = p.activeBoardId
    ? p.item.boardIds.includes(p.activeBoardId)
    : p.item.boardIds.length > 0;
  if (p.activeBoardId) {
    return (
      <button
        className={"gallery-fav" + (onActive ? " active" : "")}
        aria-label={
          onActive
            ? m.board_on_named({ name: p.item.name })
            : m.board_add_named({ name: p.item.name })
        }
        aria-pressed={onActive}
        disabled={p.disabled}
        onClick={(e) => {
          e.stopPropagation();
          p.onToggle(p.activeBoardId!);
        }}
      >
        <FolderSimple size={14} weight={onActive ? "fill" : "regular"} />
      </button>
    );
  }
  return (
    <details className={"gallery-board-menu" + (onActive ? " active" : "")}>
      <summary
        aria-label={m.board_add_named({ name: p.item.name })}
        onClick={(e) => e.stopPropagation()}
      >
        <FolderSimple size={14} weight={onActive ? "fill" : "regular"} />
      </summary>
      <div className="gallery-board-menu-list" onClick={(e) => e.stopPropagation()}>
        {p.boards.map((board) => {
          const checked = p.item.boardIds.includes(board.id);
          return (
            <label key={board.id} className="check">
              <input
                type="checkbox"
                checked={checked}
                disabled={p.disabled}
                onChange={() => p.onToggle(board.id)}
              />
              {board.name}
            </label>
          );
        })}
      </div>
    </details>
  );
}

function GalleryDetail(p: {
  id: string;
  locked: boolean;
  picker?: { limit: number; usedIds: string[] };
  selected: boolean;
  pickerFull: boolean;
  alreadyUsed: boolean;
  saveDirectory: string;
  boards: Board[];
  onToggleBoard: (item: GalleryItem, boardId: string) => Promise<void>;
  onClose: () => void;
  onChanged: () => Promise<void>;
  onUse: (ids: string[]) => Promise<void>;
  onEdit: (item: GalleryItem) => Promise<void | boolean>;
  onToggle: () => void;
  onDelete: (id: string) => void;
  error: string;
  setError: (e: string) => void;
  run: (action: () => Promise<void>) => Promise<void>;
}) {
  const [item, setItem] = useState<GalleryItem | null>(null);
  const [loadError, setLoadError] = useState("");
  const [name, setName] = useState("");
  const [tagInput, setTagInput] = useState("");
  const [copied, setCopied] = useState(false);
  const [imageBroken, setImageBroken] = useState(false);
  const retried = useRef(false);
  const currentId = useRef("");
  const locked = p.locked;

  const load = async () => {
    const id = p.id;
    currentId.current = id;
    try {
      const [found] = await api.galleryGet([id]);
      if (currentId.current !== id) return;
      if (!found) {
        setItem(null);
        setLoadError(m.backend_gallery_gone());
        return;
      }
      setItem(found);
      setLoadError("");
    } catch (e) {
      if (currentId.current === id) setLoadError(String(e));
    }
  };
  useEffect(() => {
    retried.current = false;
    setImageBroken(false);
    setCopied(false);
    setLoadError("");
    void load();
    return () => {
      currentId.current = "";
    };
  }, [p.id]);
  useEffect(() => {
    if (item) setName(item.name);
  }, [item]);

  const patch = async (change: GalleryPatch) => {
    const [updated] = await api.galleryPatch([p.id], change);
    if (updated) setItem(updated);
    await p.onChanged();
  };
  const addTags = async () => {
    const tags = parseTags(tagInput);
    if (!tags.length) return;
    await patch({ addTags: tags });
    setTagInput("");
  };
  const saveImage = async (target: GalleryItem) => {
    const ext =
      target.mime === "image/jpeg" ? "jpg" : target.mime.split("/")[1];
    const base = target.name
      .replace(/\.(png|jpe?g|webp|gif)$/i, "")
      .replace(/[<>:"/\\|?*]/g, "_");
    const path = await save({
      defaultPath: exportDefaultPath(p.saveDirectory, `${base}.${ext}`),
      filters: [{ name: m.file_image(), extensions: [ext] }],
    });
    if (path)
      await api.saveDataUrl((await api.galleryRead(target.id)).dataUrl, path);
  };
  const unavailable = item
    ? item.pendingDelete || item.availability === "missing" || imageBroken
    : true;
  return (
    <Modal
      title={item?.name ?? m.gallery()}
      large
      onClose={p.onClose}
    >
      {loadError ? (
        <div className="gallery-error" role="alert">
          {loadError}
          <button onClick={() => void load()}>{m.action_retry()}</button>
        </div>
      ) : !item ? (
        <p className="help" role="status">
          {m.gallery_loading()}
        </p>
      ) : (
        <>
          <div className="gallery-preview">
            {imageBroken || item.availability === "missing" ? (
              <div className="gallery-preview-missing">
                {item.hasThumbnail ? (
                  <img
                    src={api.galleryThumbnail(
                      item.id,
                      item.thumbnailRevision,
                    )}
                    alt={item.name}
                  />
                ) : (
                  <Images size={44} weight="thin" />
                )}
                <p>
                  {item.availability === "missing"
                    ? m.gallery_missing()
                    : m.gallery_original_unreadable()}
                </p>
              </div>
            ) : (
              <img
                src={api.assetUrl(item.filePath)}
                alt={item.name}
                onError={() => {
                  setImageBroken(true);
                  if (!retried.current) {
                    retried.current = true;
                    void load();
                  }
                }}
              />
            )}
          </div>
          <div className="gallery-detail-meta">
            <span>
              {item.width}×{item.height}
            </span>
            <span>{formatBytes(item.byteSize)}</span>
            <span>
              {item.model
                ? modelLabel(
                    item.model,
                    modelByAnyId(item.model)?.label ?? item.model,
                  )
                : m.gallery_imported_image()}
            </span>
            <span>{formatDateTime(item.createdAt)}</span>
            <span>
              {item.pendingDelete
                ? m.gallery_pending()
                : item.availability === "missing"
                  ? m.gallery_missing()
                  : m.gallery_available()}
            </span>
          </div>
          <div className="gallery-detail-name">
            <input
              aria-label={m.gallery_name()}
              value={name}
              disabled={locked || item.pendingDelete}
              onChange={(e) => setName(e.target.value)}
            />
            <button
              disabled={
                locked ||
                item.pendingDelete ||
                !name.trim() ||
                name.trim() === item.name
              }
              onClick={() =>
                void p.run(() => patch({ name: name.trim() }))
              }
            >
              {m.gallery_save_name()}
            </button>
          </div>
          <div className="gallery-detail-boards">
            {p.boards.length === 0 ? (
              <span className="muted">{m.board_empty()}</span>
            ) : (
              <>
                {p.boards
                  .filter((board) => item.boardIds.includes(board.id))
                  .map((board) => (
                    <button
                      key={board.id}
                      disabled={locked || item.pendingDelete}
                      onClick={() =>
                        void p.run(async () => {
                          await p.onToggleBoard(item, board.id);
                          await load();
                        })
                      }
                    >
                      {board.name} ×
                    </button>
                  ))}
                <label className="gallery-board-select">
                  <span className="muted">{m.board_add_to()}</span>
                  <select
                    aria-label={m.board_add()}
                    disabled={locked || item.pendingDelete}
                    value=""
                    onChange={(e) => {
                      const target = e.target.value;
                      e.target.value = "";
                      if (!target) return;
                      void p.run(async () => {
                        await p.onToggleBoard(item, target);
                        await load();
                      });
                    }}
                  >
                    <option value="">
                      {item.boardIds.length ? m.board_add() : m.board_none()}
                    </option>
                    {p.boards
                      .filter((board) => !item.boardIds.includes(board.id))
                      .map((board) => (
                        <option key={board.id} value={board.id}>
                          {board.name}
                        </option>
                      ))}
                  </select>
                </label>
              </>
            )}
          </div>
          <div className="gallery-detail-tags">
              <div className="tags">
                {item.tags.map((tagName) => (
                  <button
                    key={tagName}
                    disabled={locked || item.pendingDelete}
                    aria-label={m.gallery_tag_remove_named({ name: tagName })}
                    onClick={() =>
                      void p.run(() => patch({ removeTags: [tagName] }))
                    }
                  >
                    {tagName} ×
                  </button>
                ))}
              </div>
              <div className="row">
                <input
                  aria-label={m.gallery_tag_edit()}
                  placeholder={m.gallery_tag_placeholder()}
                  value={tagInput}
                  disabled={locked || item.pendingDelete}
                  onChange={(e) => setTagInput(e.target.value)}
                />
                <button
                  disabled={locked || item.pendingDelete || !tagInput.trim()}
                  onClick={() => void p.run(addTags)}
                >
                  {m.gallery_tag_add()}
                </button>
              </div>
          </div>
          {item.prompt &&
            (item.prompt.length > 240 ? (
              <details className="gallery-prompt">
                <summary>{m.gallery_prompt()}</summary>
                <p>{item.prompt}</p>
              </details>
            ) : (
              <div className="gallery-prompt">
                <span className="muted">{m.gallery_prompt()}</span>
                <p>{item.prompt}</p>
              </div>
            ))}
          <GenerationInfo details={item.details} />
          {p.error && (
            <p className="error-text" role="alert">
              {p.error}
            </p>
          )}
          <div className="gallery-detail-actions">
            <button
              disabled={locked}
              onClick={p.onClose}
            >
              <ArrowLeft size={16} />
              {m.gallery_back()}
            </button>
            <button
              disabled={locked || unavailable}
              onClick={() =>
                void p.run(async () => {
                  await api.copyImage(
                    (await api.galleryRead(item.id)).dataUrl,
                  );
                  setCopied(true);
                })
              }
            >
              <Copy size={16} />
              {copied ? m.gallery_copied() : m.action_copy_image()}
            </button>
            {p.picker ? (
              <button
                className="primary"
                disabled={
                  locked ||
                  unavailable ||
                  p.alreadyUsed ||
                  (!p.selected && p.pickerFull)
                }
                onClick={p.onToggle}
              >
                {p.selected ? m.gallery_unselect() : m.gallery_select_this()}
              </button>
            ) : (
              <>
                <button
                  disabled={locked || unavailable}
                  onClick={() =>
                    void p.run(async () => {
                      await p.onUse([item.id]);
                      p.onClose();
                    })
                  }
                >
                  {m.gallery_add_refs()}
                </button>
                <button
                  className="primary"
                  disabled={locked || unavailable}
                  onClick={() =>
                    void p.run(async () => {
                      if ((await p.onEdit(item)) !== false) p.onClose();
                    })
                  }
                >
                  {m.gallery_edit_this()}
                </button>
                <button
                  disabled={locked || unavailable}
                  onClick={() => void p.run(() => saveImage(item))}
                >
                  {m.action_save_as()}
                </button>
                <button
                  className="danger"
                  disabled={locked}
                  onClick={() => p.onDelete(item.id)}
                >
                  <Trash size={16} />
                  {m.action_delete()}
                </button>
              </>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}
