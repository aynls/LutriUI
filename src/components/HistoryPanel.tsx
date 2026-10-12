import { useState } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { ArrowClockwise, ArrowLeft, Trash } from "@phosphor-icons/react";
import { assetUrl, galleryThumbnail, historyDelete, importImage, saveDataUrl } from "../lib/api";
import type { HistoryItem } from "../lib/types";
import { exportDefaultPath } from "../lib/export";
import { localizeStored, m, formatDateTime } from "../i18n";
import {
  fieldLabel,
  historyPhase,
  modelLabel,
  providerLabel,
  requestStatus,
} from "../labels";
import { catalog, modelByAnyId } from "../models/catalog";
import Modal from "./Modal";
import { displayValue } from "../workspaces/shared/Controls";

interface Props {
  onCancel?: (id: string) => void;
  saveDirectory: string;
  items: HistoryItem[];
  onRefresh: () => void;
  onUseAsInput: (item: HistoryItem) => void;
  onRestoreEdit: (item: HistoryItem) => void;
}

export default function HistoryPanel({
  onCancel,
  saveDirectory,
  items,
  onRefresh,
  onUseAsInput,
  onRestoreEdit,
}: Props) {
  const [error, setError] = useState("");
  const [detailId, setDetailId] = useState<string | null>(null);
  const detail = items.find((it) => it.id === detailId) ?? null;

  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [deleteIds, setDeleteIds] = useState<string[]>([]);
  const [deleting, setDeleting] = useState(false);
  const canDelete = (it: HistoryItem) =>
    it.status !== "queued" && it.status !== "running";
  const deletable = items.filter(canDelete);
  const selected = deletable.filter((it) => selectedIds.has(it.id));
  const toggle = (id: string) =>
    setSelectedIds((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const confirmDelete = async () => {
    if (deleting) return;
    setDeleting(true);
    setError("");
    const failed: string[] = [];
    const removed = new Set<string>();
    try {
      for (const id of deleteIds) {
        try {
          await historyDelete(id);
          removed.add(id);
        } catch (e) {
          failed.push(String(e));
        }
      }
      if (removed.has(detailId ?? "")) setDetailId(null);
      setSelectedIds(
        (previous) => new Set([...previous].filter((id) => !removed.has(id))),
      );
      setDeleteIds([]);
      if (failed.length)
        setError(
          m.error_delete_records({ count: failed.length, detail: failed[0] }),
        );
      else {
        setBatchMode(false);
        setSelectedIds(new Set());
      }
      onRefresh();
    } finally {
      setDeleting(false);
    }
  };
  const confirmation = deleteIds.length > 0 && (
    <Modal
      title={m.history_title_delete()}
      onClose={() => {
        if (!deleting) setDeleteIds([]);
      }}
    >
      <p>
        {m.history_delete_body({ count: deleteIds.length })}
      </p>
      <div className="history-delete-actions">
        <button disabled={deleting} onClick={() => setDeleteIds([])}>
          {m.action_cancel()}
        </button>
        <button
          className="danger"
          disabled={deleting}
          onClick={() => void confirmDelete()}
        >
          {deleting ? m.history_deleting() : m.history_confirm()}
        </button>
      </div>
    </Modal>
  );
  if (detail) {
    return (
      <>
        <HistoryDetail
          saveDirectory={saveDirectory}
          item={detail}
          onCancel={() => onCancel?.(detail.id)}
          onBack={() => setDetailId(null)}
          onUseAsInput={(index = 0) => {
            onUseAsInput({
              ...detail,
              resultFiles: [detail.resultFiles[index]],
            });
            setDetailId(null);
          }}
          onRestoreEdit={() => {
            onRestoreEdit(detail);
            setDetailId(null);
          }}
          onDelete={() => {
            setError("");
            setDeleteIds([detail.id]);
          }}
          error={error}
        />
        {confirmation}
      </>
    );
  }
  return (
    <>
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="history-list-toolbar">
          <span className="text-xs text-zinc-500">
            {m.history_total({ count: items.length })}
          </span>
          <div className="row">
            <button
              disabled={!deletable.length && !batchMode}
              onClick={() => {
                setBatchMode(!batchMode);
                setSelectedIds(new Set());
                setError("");
              }}
            >
              {batchMode ? m.history_cancel_select() : m.history_batch()}
            </button>
            <button onClick={onRefresh}>
              <ArrowClockwise size={14} /> {m.action_refresh()}
            </button>
          </div>
        </div>
        {batchMode && (
          <div className="history-batch-toolbar">
            <button
              disabled={!deletable.length}
              onClick={() =>
                setSelectedIds(
                  selected.length === deletable.length
                    ? new Set()
                    : new Set(deletable.map((it) => it.id)),
                )
              }
            >
              {selected.length > 0 && selected.length === deletable.length
                ? m.history_select_none()
                : m.history_select_all()}
            </button>
            <span className="muted">
              {m.history_selected({ count: selected.length })}
            </span>
            <button
              className="danger"
              disabled={!selected.length}
              onClick={() => setDeleteIds(selected.map((it) => it.id))}
            >
              {m.history_delete_selected()}
            </button>
          </div>
        )}
        {error && (
          <p role="alert" className="error-text history-list-error">
            {error}
          </p>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto p-3">
          {items.length === 0 ? (
            <p className="text-xs text-zinc-500">{m.history_empty()}</p>
          ) : (
            <div className="flex flex-col gap-2">
              {items.map((it, index) => (
                <div
                  key={it.id}
                  className={
                    "history-row" +
                    (batchMode && selectedIds.has(it.id) ? " selected" : "")
                  }
                >
                  {batchMode && (
                    <input
                      type="checkbox"
                      aria-label={m.history_select_n({ index: index + 1 })}
                      checked={selectedIds.has(it.id) && canDelete(it)}
                      disabled={!canDelete(it)}
                      onChange={() => toggle(it.id)}
                    />
                  )}
                  <button
                    className="history-row-content"
                    disabled={batchMode && !canDelete(it)}
                    onClick={() =>
                      batchMode ? toggle(it.id) : setDetailId(it.id)
                    }
                  >
                    {it.thumbnailAssetId || it.thumb ? (
                      <img
                        src={
                          it.thumbnailAssetId
                            ? galleryThumbnail(it.thumbnailAssetId)
                            : it.thumb!
                        }
                        alt=""
                        className="h-14 w-14 shrink-0 rounded object-cover"
                      />
                    ) : (
                      <div className="flex h-14 w-14 shrink-0 items-center justify-center rounded bg-zinc-800 text-[10px] text-zinc-500">
                        {historyStatus(it)}
                      </div>
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5 text-[10px] text-zinc-500">
                        <span className="rounded bg-zinc-800 px-1">
                          {it.mode === "edit" ? m.task_edit() : m.mode_text()}
                        </span>
                        <span>{formatDateTime(it.createdAt)}</span>
                        {(it.cost !== null || it.usage?.credits != null) && (
                          <span className="text-emerald-500">
                            {it.provider === "comfy"
                              ? m.history_credits({
                                  value:
                                    it.usage?.credits ?? m.state_not_returned(),
                                })
                              : it.cost !== null
                                ? `$${it.cost.toFixed(3)}`
                                : m.history_cost_missing()}
                          </span>
                        )}
                      </div>
                      <p className="mt-0.5 line-clamp-2 text-xs leading-relaxed text-zinc-300">
                        {it.prompt || m.history_no_prompt()}
                      </p>
                      <span className="muted">
                        {modelLabel(
                          it.model,
                          modelByAnyId(it.model)?.label ?? it.model,
                        )}{" "}
                        · {historyStatus(it)}
                      </span>
                    </div>
                  </button>
                  {!batchMode && canDelete(it) && (
                    <button
                      className="history-row-delete danger"
                      aria-label={m.history_delete_n({ index: index + 1 })}
                      title={m.history_delete_record()}
                      onClick={() => {
                        setError("");
                        setDeleteIds([it.id]);
                      }}
                    >
                      <Trash size={18} />
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
      {confirmation}
    </>
  );
}

function HistoryDetail({
  onCancel,
  saveDirectory,
  item,
  onBack,
  onUseAsInput,
  onRestoreEdit,
  onDelete,
  error,
}: {
  onCancel: () => void;
  saveDirectory: string;
  item: HistoryItem;
  onBack: () => void;
  onUseAsInput: (index?: number) => void;
  onRestoreEdit: () => void;
  onDelete: () => void;
  error: string;
}) {
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [resultIndex, setResultIndex] = useState(0);
  const [preview, setPreview] = useState<{ src: string; title: string } | null>(
    null,
  );
  const resultFile = item.resultFiles[resultIndex];
  const resultSrc = resultFile ? assetUrl(resultFile) : null;
  const inputSrc = item.inputFiles[0] ? assetUrl(item.inputFiles[0]) : null;

  const saveAs = async () => {
    if (!resultFile) return;
    const ext = /\.(jpe?g|webp)$/i.exec(resultFile)?.[1].toLowerCase() ?? "png";
    const path = await saveDialog({
      defaultPath: exportDefaultPath(
        saveDirectory,
        `lutriui-${item.id.slice(0, 8)}${resultIndex ? "-" + (resultIndex + 1) : ""}.${ext}`,
      ),
      filters: [{ name: m.file_image(), extensions: [ext] }],
    });
    if (!path) return;
    setSaving(true);
    try {
      const img = await importImage(resultFile);
      await saveDataUrl(img.dataUrl, path);
    } catch (e) {
      setSaveError(m.error_save_failed({ detail: String(e) }));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="history-detail flex min-h-0 flex-1 flex-col overflow-y-auto p-3">
      <button
        className="history-back mb-2 self-start text-xs text-zinc-400 hover:text-zinc-200"
        onClick={onBack}
      >
        <ArrowLeft size={12} /> {m.history_back()}
      </button>

      {resultSrc && (
        <button
          className="history-image-button"
          aria-label={m.history_zoom()}
          onClick={() =>
            setPreview({ src: resultSrc, title: m.history_result_title() })
          }
        >
          <img
            src={resultSrc}
            alt={m.history_result_alt()}
            className="w-full rounded-md border border-zinc-800"
          />
        </button>
      )}
      {!resultSrc && item.resultFiles.length > 0 && (
        <p className="help">{m.history_image_removed()}</p>
      )}
      {item.resultFiles.length > 1 && (
        <div className="result-gallery mt-2" aria-label={m.history_results()}>
          {item.resultFiles.map((path, i) => (
            <button
              key={i}
              className={resultIndex === i ? "active" : ""}
              aria-label={m.history_result_n({ index: i + 1 })}
              onClick={() => setResultIndex(i)}
            >
              {path ? (
                <img
                  src={assetUrl(path)}
                  alt={m.history_result_n({ index: i + 1 })}
                />
              ) : (
                <span className="help">{m.history_image_gone()}</span>
              )}
            </button>
          ))}
        </div>
      )}

      <div className="mt-3 grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs">
        <Meta label={m.history_status()} value={historyStatus(item)} />
        <Meta
          label={m.history_mode()}
          value={item.mode === "edit" ? m.mode_image_edit() : m.mode_text()}
        />
        <Meta label={m.history_time()} value={formatDateTime(item.createdAt)} />
        <Meta label={m.history_provider()} value={providerLabel(item.provider)} />
        <Meta
          label={m.history_model()}
          value={modelLabel(
            item.model,
            modelByAnyId(item.model)?.label ?? item.model,
          )}
        />
        {(item.provider !== "comfy" || item.usage?.credits != null) && (
          <Meta
            label={m.history_cost()}
            value={
              item.provider === "comfy"
                ? item.usage?.credits != null
                  ? m.history_credits({ value: item.usage.credits })
                  : ""
                : item.cost !== null
                  ? `$${item.cost.toFixed(4)}`
                  : "-"
            }
          />
        )}
        <Meta
          label="Tokens"
          value={
            item.usage?.total_tokens ? String(item.usage.total_tokens) : "-"
          }
        />
        <Meta
          label={m.history_canvas()}
          value={
            item.canvasWidth ? `${item.canvasWidth}×${item.canvasHeight}` : "-"
          }
        />
        <Meta
          label={m.history_boxes()}
          value={m.history_box_count({ count: (item.boxes ?? []).length })}
        />
        {item.batch && (
          <Meta
            label={m.history_output_count()}
            value={m.history_output_ratio({
              done: item.resultFiles.length,
              total: item.batch.requests.length,
            })}
          />
        )}
      </div>
      {item.error && (
        <p className="error-text">{localizeStored(item.error)}</p>
      )}
      {item.taskId && (
        <p className="help">{m.history_provider_task({ id: item.taskId })}</p>
      )}
      {(item.status === "queued" || item.status === "running") && (
        <button type="button" onClick={onCancel}>
          {m.action_cancel()}
        </button>
      )}

      <div className="mt-3">
        <p className="mb-1 text-xs text-zinc-500">{m.history_prompt()}</p>
        <p className="whitespace-pre-wrap rounded border border-zinc-800 bg-zinc-900 p-2 text-xs leading-relaxed text-zinc-300">
          {item.prompt || m.history_prompt_empty()}
        </p>
      </div>

      {(item.boxes ?? []).length > 0 && (
        <div className="mt-3">
          <p className="mb-1 text-xs text-zinc-500">{m.history_boxes_px()}</p>
          <div className="flex flex-col gap-1">
            {(
              item.boxes as {
                id: string;
                role: string;
                rect: { x: number; y: number; w: number; h: number };
                desc: string;
              }[]
            ).map((b) => (
              <div
                key={b.id}
                className="rounded border border-zinc-800 px-2 py-1 font-mono text-[10px] text-zinc-400"
              >
                {b.id} [{Math.round(b.rect.x)}, {Math.round(b.rect.y)},{" "}
                {Math.round(b.rect.w)}, {Math.round(b.rect.h)}] {b.desc}
              </div>
            ))}
          </div>
        </div>
      )}
      <details className="mt-3">
        <summary>{m.history_params()}</summary>
        <dl className="request-parameters">
          {Object.entries(item.params)
            .filter(
              ([key, value]) =>
                catalog.fields[key] &&
                value != null &&
                !(item.batch && key === "count") &&
                !(
                  item.batch &&
                  item.batch.requests.length > 1 &&
                  key === "seed"
                ),
            )
            .map(([key, value]) => (
              <div key={key}>
                <dt>{fieldLabel(key, catalog.fields[key].label)}</dt>
                <dd>{displayValue(key, value as string | number | boolean)}</dd>
              </div>
            ))}
        </dl>
      </details>

      {item.batch && item.batch.requests.length > 1 && (
        <details className="mt-3">
          <summary>{m.history_requests()}</summary>
          {item.batch.requests.map((request, i) => (
            <div key={request.requestId} className="help">
              {m.history_request_n({
                index: i + 1,
                status: requestStatus(request.status),
              })}
              {request.seed != null && m.history_seed({ seed: request.seed })}
              {request.taskId && (
                <p>{m.history_provider_task({ id: request.taskId })}</p>
              )}
              {request.error && (
                <p className="error-text">{localizeStored(request.error)}</p>
              )}
            </div>
          ))}
        </details>
      )}
      <details className="mt-3">
        <summary className="cursor-pointer text-xs text-zinc-500">
          {m.history_final_prompt()}
        </summary>
        <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded border border-zinc-800 bg-zinc-900 p-2 text-[10px] text-zinc-500">
          {item.finalPrompt}
        </pre>
      </details>

      {inputSrc && (
        <div className="mt-3">
          <p className="mb-1 text-xs text-zinc-500">
            {m.history_inputs({ count: item.inputFiles.length })}
          </p>
          <div className="result-gallery">
            {item.inputFiles.map((path, i) => (
              <button
                key={path}
                aria-label={m.history_view_input({ index: i + 1 })}
                onClick={() =>
                  setPreview({
                    src: assetUrl(path),
                    title:
                      item.recipe?.refNames[i] ??
                      m.name_material_n({ index: i + 1 }),
                  })
                }
              >
                <img
                  src={assetUrl(path)}
                  alt={
                    item.recipe?.refNames[i] ??
                    m.name_material_n({ index: i + 1 })
                  }
                />
              </button>
            ))}
          </div>
        </div>
      )}

      {(error || saveError) && (
        <p role="alert" className="error-text">
          {error || saveError}
        </p>
      )}
      <div className="mt-4 grid grid-cols-2 gap-2 pb-2">
        <button
          className="rounded-md border border-zinc-700 px-3 py-2 text-xs hover:border-zinc-500"
          onClick={() => onUseAsInput(resultIndex)}
          disabled={!resultSrc}
        >
          {m.history_continue()}
        </button>
        <button
          className="rounded-md border border-zinc-700 px-3 py-2 text-xs hover:border-zinc-500"
          onClick={onRestoreEdit}
        >
          {m.history_restore()}
        </button>
        <button
          className="rounded-md border border-zinc-700 px-3 py-2 text-xs hover:border-zinc-500"
          onClick={saveAs}
          disabled={!resultSrc || saving}
        >
          {saving ? m.history_saving() : m.history_save_as()}
        </button>
        <button
          className="rounded-md border border-red-900/60 px-3 py-2 text-xs text-red-400 hover:border-red-700"
          onClick={onDelete}
          disabled={item.status === "queued" || item.status === "running"}
        >
          <Trash size={12} /> {m.history_delete_record()}
        </button>
      </div>
      {preview && (
        <Modal title={preview.title} onClose={() => setPreview(null)} large>
          <div className="image-preview">
            <img src={preview.src} alt={preview.title} />
          </div>
        </Modal>
      )}
    </div>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-1.5">
      <span className="shrink-0 text-zinc-500">{label}</span>
      <span className="truncate text-zinc-300" title={value}>
        {value}
      </span>
    </div>
  );
}
function historyStatus(item: HistoryItem) {
  const status =
    item.status === "running"
      ? historyPhase(item.phase ?? "")
      : requestStatus(item.status);
  return item.batch && item.batch.requests.length > 1
    ? m.status_with_count({
        status,
        done: item.batch.requests.filter((request) => request.status === "ok")
          .length,
        total: item.batch.requests.length,
      })
    : status;
}
