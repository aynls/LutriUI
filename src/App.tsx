import { useGenerationTasks } from "./app/useGenerationTasks";
import { useMcpBridge } from "./app/useMcpBridge";
import {
  createApplicationActions,
  type ApplicationHost,
} from "./app/applicationActions";
import { McpError } from "./app/workspaceActions";
import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  ArrowCounterClockwise,
  ArrowClockwise,
  GearSix,
  Plus,
  X,
} from "@phosphor-icons/react";
import ResizableSidebar from "./components/ResizableSidebar";
import HistoryPanel from "./components/HistoryPanel";
import Gallery from "./components/Gallery";
import GenerationGrid from "./components/GenerationGrid";
import EditHandoff from "./components/EditHandoff";
import { regionsEnabled } from "./models/flux/layout";
import Settings from "./components/Settings";
import Modal from "./components/Modal";
import References from "./workspaces/shared/References";
import ResizableReferences from "./components/ResizableReferences";
import { WorkspaceSidebar, WorkspaceStage } from "./workspaces";
import { useWorkspace } from "./app/useWorkspace";
import * as api from "./lib/api";
import { rebaseHistoryPaths } from "./lib/library";
import { imageSize, makeThumb } from "./lib/image";
import { exportDefaultPath } from "./lib/export";
import {
  renameBox,
  withIds,
  validateDraft,
  readDraft,
  taskIntent,
  workspaceKey,
  primaryImage,
  setPrimaryImage,
  beginImageEdit,
} from "./lib/workspace";
import { familyById, routeFor, singleImageDraft } from "./models/catalog";
import { compileDraft } from "./models";
import { maskFromRects } from "./workspaces/gpt/mask";
import type {
  Box,
  HistoryItem,
  ProviderStatus,
  WorkingImage,
  GalleryItem,
  LibraryMaintainOp,
  Draft,
} from "./lib/types";

import {
  getResultBase,
  workingImageFromResult,
  type SavedResult,
} from "./app/generation";
import { ResultStage, ResultActions } from "./components/Results";
import { updateFluxBoxes } from "./models/flux/regions";
import { m } from "./i18n";

export default function App() {
  const [notice, setNotice] = useState("");
  const transientNotice = useRef("");
  const showNotice = useCallback((text: string, transient = false) => {
    transientNotice.current = transient ? text : "";
    setNotice(text);
  }, []);
  useEffect(() => {
    if (!transientNotice.current || notice !== transientNotice.current) return;
    const timer = window.setTimeout(
      () => setNotice((current) => (current === notice ? "" : current)),
      4000,
    );
    return () => window.clearTimeout(timer);
  }, [notice]);
  const [closeRequested, setCloseRequested] = useState(false);
  const requestClose = useCallback(() => setCloseRequested(true), []);
  const submitting = useRef(false);
  const [storageMigrating, setStorageMigrating] = useState(false);
  const migrating = useRef(false);
  const ws = useWorkspace(submitting, showNotice, requestClose, storageMigrating);
  const {
    prefs,
    setPrefs,
    draft,
    current,
    ready,
    commit,
    onChange,
    undo,
    redo,
    undoStack,
    redoStack,
  } = ws;
  const [selectedId, setSelected] = useState<string | null>(null);
  const [pStatus, setPStatus] = useState<ProviderStatus | null>(null);

  const [historyError, setHistoryError] = useState("");
  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [tab, setTab] = useState<"params" | "history">("params");
  const [galleryOpen, setGalleryOpen] = useState(false);
  const [galleryPicker, setGalleryPicker] = useState(false);
  const [editHandoff, setEditHandoff] = useState<{
    draft: Draft;
    image: WorkingImage;
    decide: (keep: boolean | null) => void;
  } | null>(null);
  const [galleryRevision, setGalleryRevision] = useState(0);
  const [galleryIds, setGalleryIds] = useState<string[] | undefined>(undefined);
  const galleryContext = useRef(false);
  galleryContext.current = galleryOpen || galleryPicker;
  const [settings, setSettings] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [requestFeedback, setRequestFeedback] = useState<{
    key: string;
    id: string;
  } | null>(null);
  const [refPreview, setRefPreview] = useState<string | null>(null);
  const [urlDialog, setUrlDialog] = useState(false),
    [imageUrl, setImageUrl] = useState("");
  const [importing, setImporting] = useState(false);
  const importQueue = useRef(Promise.resolve());
  const importJobs = useRef(0);
  const [results, setResults] = useState<Partial<Record<string, SavedResult>>>(
    {},
  );
  const currentResults = useRef(results);
  currentResults.current = results;
  const intent = taskIntent(draft);
  const activeKey = workspaceKey(draft);
  const activeLayout = regionsEnabled(draft);
  const ordinaryGeneration = intent === "create" && !activeLayout;
  const result = results[activeKey] ?? null;
  const [attempts, setAttempts] = useState<
    Partial<Record<string, SavedResult[]>>
  >({});
  const [view, setView] = useState<"canvas" | "result" | "compare">("canvas");
  const [resultPreview, setResultPreview] = useState(false);
  const resultSelectionEpoch = useRef(0);
  const previewAttempts = attempts[activeKey] ?? [];
  const canClosePreview = previewAttempts.every(
    (attempt) => attempt.saved && attempt.item.status !== "running",
  );
  const closeGenerationPreview = () => {
    ++resultSelectionEpoch.current;
    currentResults.current = {
      ...currentResults.current,
      [activeKey]: undefined,
    };
    setResults(currentResults.current);
    setAttempts((previous) => ({ ...previous, [activeKey]: [] }));
    setResultPreview(false);
    setView("canvas");
  };
  const [original, setOriginal] = useState(false),
    [savingResult, setSavingResult] = useState(false);
  const storeResult = (r: SavedResult, preserveSelection = false) => {
    const key = workspaceKey(r.snapshot);
    const selected = (previous: SavedResult | undefined) => {
      const next =
        preserveSelection && previous?.item.id === r.item.id
          ? {
              ...r,
              selectedIndex: previous.selectedIndex,
              image: previous.image,
            }
          : r;
      return {
        ...next,
        image: {
          ...next.image,
          assetId: next.item.resultAssetIds?.[next.selectedIndex],
        },
      };
    };
    currentResults.current = {
      ...currentResults.current,
      [key]: selected(currentResults.current[key]),
    };
    setResults(currentResults.current);
    setAttempts((s) => ({
      ...s,
      [key]: s[key]?.some((v) => v.item.id === r.item.id)
        ? s[key]!.map((v) => (v.item.id === r.item.id ? selected(v) : v))
        : [...(s[key] ?? []), selected(undefined)].slice(-20),
    }));
  };
  const markSaved = (r: SavedResult) => {
    const key = workspaceKey(r.snapshot);
    setResults((s) =>
      s[key]?.item.id === r.item.id
        ? {
            ...s,
            [key]: {
              ...s[key]!,
              item: r.item,
              saved: true,
              image: {
                ...s[key]!.image,
                assetId: r.item.resultAssetIds?.[s[key]!.selectedIndex],
              },
            },
          }
        : s,
    );
    setAttempts((s) => ({
      ...s,
      [key]: (s[key] ?? []).map((v) =>
        v.item.id === r.item.id
          ? {
              ...v,
              item: r.item,
              saved: true,
              image: {
                ...v.image,
                assetId: r.item.resultAssetIds?.[v.selectedIndex],
              },
            }
          : v,
      ),
    }));
  };
  const refreshProviders = useCallback(async () => {
    setPStatus(await api.providerStatus());
  }, []);
  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await api.historyList());
      setHistoryError("");
    } catch (e) {
      setHistoryError(m.notice_history_read_failed({ detail: String(e) }));
    }
    setGalleryRevision((n) => n + 1);
  }, []);
  const generation = useGenerationTasks(
    submitting,
    (r, publish) => {
      const key = workspaceKey(r.snapshot);
      const current = currentResults.current[key];
      if (publish?.settleOnly && current?.item.id !== r.item.id) {
        setAttempts((previous) => {
          const list = previous[key];
          if (!list?.some((attempt) => attempt.item.id === r.item.id))
            return previous;
          return {
            ...previous,
            [key]: list.map((attempt) =>
              attempt.item.id === r.item.id
                ? {
                    ...r,
                    selectedIndex: attempt.selectedIndex,
                    image: attempt.image,
                  }
                : attempt,
            ),
          };
        });
      } else storeResult(r, true);
      if (r.item.status === "running" || publish?.quiet || publish?.settleOnly)
        return;
      // Task completion never interrupts the canvas or the user's next request.
      if (r.saved && r.item.status !== "partial")
        showNotice(m.notice_task_saved(), true);
      else
        showNotice(
          r.saved ? m.notice_batch_partial() : m.notice_task_unsaved(),
        );
    },
    refreshHistory,
    showNotice,
  );
  const generationTask =
    generation.tasks.find((t) => (t.intent ?? "create") === intent) ??
    generation.task;
  const busy = generation.inflight > 0;
  const storageBusy =
    !ready ||
    !generation.ready ||
    generation.accepting ||
    generation.inflight > 0 ||
    importing ||
    savingResult;
  const storageBusyRef = useRef(storageBusy);
  storageBusyRef.current = storageBusy;
  // The MCP bridge acts on the live workspace through the same host callbacks
  // the canvas uses; actions stay transport-independent in applicationActions.
  const hostReady = useRef(false);
  hostReady.current = ready && generation.ready;
  // Running tasks never block agent edits; only migration, import/save and
  // submission admission do.
  const storageLocked = useRef(false);
  storageLocked.current =
    importing || savingResult || generation.accepting;
  const submitLocked = useRef(false);
  submitLocked.current =
    importing || savingResult || generation.accepting;
  const mcpHost = useRef<ApplicationHost | null>(null);
  mcpHost.current = {
    ready: () => hostReady.current,
    getVersion: ws.getVersion,
    isGestureActive: ws.isGestureActive,
    canPersist: ws.canPersist,
    isBusy: () => migrating.current || storageLocked.current,
    submitBlocked: () => migrating.current || submitLocked.current,
    readCurrent: () => current.current,
    readIntent: (task) => ws.draftForIntent(task),
    commit: (next) => {
      flushSync(() => ws.commit(next, true));
      showNotice(m.notice_agent_updated(), true);
    },
    persist: (snapshot) => ws.persist(snapshot),
    submit: (snapshot, count, mcpSubmission) =>
      generation.submit(snapshot, count, mcpSubmission),
    stopRemaining: (id) => generation.stopRemaining(id),
    mcpSubmissionGet: (key) => api.mcpSubmissionGet(key),
    readAsset: (id) => api.galleryRead(id),
    maskFromRects,
    makePreview: (dataUrl) => makeThumb(dataUrl, 384),
    providerStatus: () =>
      pStatus as unknown as Record<string, boolean> | null,
  };
  const mcpActions = useRef(createApplicationActions(() => mcpHost.current!));
  // Registration advertises readiness only once workspace and generation
  // restoration finished; unmount/unready removes the matching instance.
  useMcpBridge(mcpActions.current, ready && generation.ready);
  useEffect(() => {
    if (!api.isDesktop()) return;
    let off: (() => void) | undefined;
    let disposed = false;
    void listen("mcp-library-changed", () => {
      void refreshHistory();
    }).then((fn) => {
      if (disposed) fn();
      else off = fn;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [refreshHistory]);
  const storageBlocked = () => {
    if (!migrating.current) return false;
    showNotice(m.backend_storage_migrating(), true);
    return true;
  };
  const onMigrate = async (path: string): Promise<api.LibraryMigration> => {
    if (migrating.current) throw new Error(m.backend_storage_migrating());
    if (storageBusyRef.current || submitting.current || importJobs.current > 0)
      throw new Error(m.backend_library_busy());
    migrating.current = true;
    setStorageMigrating(true);
    try {
      await ws.persist();
      const moved = await api.libraryMigrate(path);
      const rebase = (r: SavedResult): SavedResult => ({
        ...r,
        item: rebaseHistoryPaths(r.item, moved.previousPath, moved.path),
      });
      currentResults.current = Object.fromEntries(
        Object.entries(currentResults.current).map(([key, value]) => [
          key,
          value && rebase(value),
        ]),
      );
      setResults(currentResults.current);
      setAttempts((s) =>
        Object.fromEntries(
          Object.entries(s).map(([key, value]) => [key, value?.map(rebase)]),
        ),
      );
      await refreshHistory();
      return moved;
    } finally {
      migrating.current = false;
      setStorageMigrating(false);
    }
  };
  const onMaintain = async (operation: LibraryMaintainOp) => {
    if (migrating.current) throw new Error(m.backend_storage_migrating());
    if (storageBusyRef.current || submitting.current || importJobs.current > 0)
      throw new Error(m.backend_library_busy());
    migrating.current = true;
    setStorageMigrating(true);
    try {
      await ws.persist();
      return await api.libraryMaintain(operation);
    } finally {
      migrating.current = false;
      setStorageMigrating(false);
      await refreshHistory();
    }
  };
  useEffect(() => {
    void refreshProviders().catch((e) =>
      showNotice(m.notice_provider_status_failed({ detail: String(e) })),
    );
    void refreshHistory();
  }, [refreshProviders, refreshHistory]);
  const refAssetKey = draft.refs.map((r) => r.assetId ?? "").join(",");
  useEffect(() => {
    const ids = draft.refs.flatMap((r) => (r.assetId ? [r.assetId] : []));
    if (!ids.length) {
      setGalleryIds(undefined);
      return;
    }
    let stale = false;
    api
      .galleryGet(ids)
      .then((items) => {
        if (stale) return;
        setGalleryIds(
          items
            .filter(
              (item) =>
                !item.pendingDelete && item.availability === "available",
            )
            .map((item) => item.id),
        );
      })
      .catch(() => {
        if (!stale) setGalleryIds(undefined);
      });
    return () => {
      stale = true;
    };
  }, [refAssetKey, galleryRevision]);
  useEffect(() => {
    setSelected(null);
    setView(
      intent === "create" && draft.family !== "flux" && result
        ? "result"
        : "canvas",
    );
    setRefPreview(null);
    setResultPreview(false);
    resultSelectionEpoch.current++;
  }, [activeKey]);
  useEffect(() => {
    if (activeLayout) setView("canvas");
  }, [activeLayout]);

  const importBatch = useCallback(
    (
      loaders: (() => Promise<api.ImportedImage>)[],
      source: "file" | "clipboard" | "url" = "file",
    ) => {
      if (storageBlocked()) return;
      const destination = galleryContext.current ? "gallery" : "references";
      const target = workspaceKey(current.current);
      setImporting(true);
      importJobs.current++;
      importQueue.current = importQueue.current
        .catch(() => {})
        .then(async () => {
          const accepted: WorkingImage[] = [],
            errors: string[] = [];
          for (const load of loaders) {
            const max = routeFor(current.current)?.maxRefs ?? 0;
            if (
              destination === "references" &&
              current.current.refs.length + accepted.length >= max
            ) {
              errors.push(m.error_refs_truncated({ count: max }));
              break;
            }
            try {
              const r = await load();
              if (r.width * r.height > 64_000_000)
                throw new Error(m.error_over_64mp({ name: r.name }));
              const assetId =
                r.assetId ?? (await api.galleryImport(r, source)).id;
              accepted.push({ ...r, assetId, uid: crypto.randomUUID() });
            } catch (e) {
              errors.push(String(e));
            }
          }
          if (
            accepted.length &&
            destination === "references" &&
            workspaceKey(current.current) !== target
          )
            errors.push(
              m.error_task_switched_gallery(),
            );
          let added = 0;
          if (
            accepted.length &&
            destination === "references" &&
            workspaceKey(current.current) === target
          ) {
            const room = Math.max(
              0,
              (routeFor(current.current)?.maxRefs ?? 0) -
                current.current.refs.length,
            );
            if (accepted.length > room)
              errors.push(m.error_import_limit_changed());
            const next = {
              ...current.current,
              refs: [...current.current.refs, ...accepted.slice(0, room)],
            };
            added = accepted.slice(0, room).length;
            commit(
              taskIntent(next) === "edit" && !next.baseId && next.refs.length
                ? setPrimaryImage(next, next.refs[0].uid!)
                : next,
              true,
            );
          }
          const summary = added
            ? m.notice_refs_added({ count: added })
            : accepted.length
              ? m.notice_images_imported({ count: accepted.length })
              : "";
          showNotice(
            [summary, ...errors].filter(Boolean).join("；"),
            Boolean(summary) && errors.length === 0,
          );
          await refreshHistory();
        })
        .catch((e) =>
          showNotice(m.notice_import_failed({ detail: String(e) })),
        )
        .finally(() => {
          importJobs.current--;
          if (!importJobs.current) setImporting(false);
        });
    },
    [commit, current, refreshHistory, showNotice],
  );
  const openFiles = async () => {
    try {
      const paths = await open({
        multiple: true,
        filters: [
          { name: m.file_image(), extensions: ["png", "jpg", "jpeg", "webp", "gif"] },
        ],
      });
      if (paths)
        importBatch(
          (Array.isArray(paths) ? paths : [paths]).map(
            (path) => () => api.importImage(path),
          ),
        );
    } catch (e) {
      showNotice(String(e));
    }
  };
  const importMask = async () => {
    const snapshot = current.current;
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: m.file_mask(), extensions: ["png"] }],
      });
      if (!path) return;
      const mask = await api.importImage(Array.isArray(path) ? path[0] : path);
      if (
        current.current.family !== snapshot.family ||
        current.current.refs[0]?.uid !== snapshot.refs[0]?.uid
      )
        throw new Error(m.error_mask_target_changed());
      commit(
        {
          ...current.current,
          mask: { ...mask, uid: crypto.randomUUID() },
          maskRects: [],
        },
        true,
      );
    } catch (e) {
      showNotice(m.notice_mask_import_failed({ detail: String(e) }));
    }
  };
  useEffect(() => {
    if (!api.isDesktop()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "drop")
          importBatch(
            e.payload.paths.map((path) => () => api.importImage(path)),
          );
      })
      .then((fn) => {
        if (disposed) fn();
        else off = fn;
      })
      .catch((e) =>
        showNotice(m.notice_drop_init_failed({ detail: String(e) })),
      );
    return () => {
      disposed = true;
      off?.();
    };
  }, [importBatch]);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const dialogs = Array.from(document.querySelectorAll("dialog[open]"));
      if (
        dialogs.length &&
        !(
          dialogs.length === 1 &&
          dialogs[0].getAttribute("aria-label") === m.pick_from_gallery() &&
          (e.ctrlKey || e.metaKey) &&
          e.key.toLowerCase() === "v"
        )
      )
        return;

      if (!ready || importing || settings || refPreview || urlDialog) return;
      const typing =
        e.target instanceof HTMLElement &&
        (["INPUT", "TEXTAREA", "SELECT"].includes(e.target.tagName) ||
          e.target.isContentEditable);
      if (
        !galleryContext.current &&
        !typing &&
        (e.ctrlKey || e.metaKey) &&
        e.key.toLowerCase() === "z"
      ) {
        e.preventDefault();
        e.shiftKey ? redo() : undo();
        setSelected(null);
      }
      if (
        !galleryContext.current &&
        !typing &&
        e.ctrlKey &&
        e.key.toLowerCase() === "y"
      ) {
        e.preventDefault();
        redo();
        setSelected(null);
      }
      if (!typing && (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") {
        e.preventDefault();
        importBatch([api.clipboardImage], "clipboard");
      }
      if (
        !galleryContext.current &&
        !typing &&
        ["Delete", "Backspace"].includes(e.key) &&
        selectedId
      ) {
        e.preventDefault();
        const d = current.current;
        if (d.family === "gpt" && d.refs[0]) {
          const rects = (d.maskRects ?? []).filter(
            (_, i) => "编辑区_" + (i + 1) !== selectedId,
          );
          commit(
            {
              ...d,
              maskRects: rects,
              mask: rects.length ? maskFromRects(d.refs[0], rects) : null,
            },
            true,
          );
        } else if (d.family === "flux")
          commit(
            { ...d, boxes: d.boxes.filter((b) => b.id !== selectedId) },
            true,
          );
        setSelected(null);
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });

  const requestDraft = singleImageDraft(draft);
  const preview = compileDraft(requestDraft);
  const errors = [
    ...validateDraft(requestDraft),
    ...(draft.prompt.trim() || draft.boxes.length
      ? preview.error
        ? [preview.error]
        : []
      : []),
  ];
  const retryHistory = async (r: SavedResult) => {
    if (storageBlocked()) return;
    setSavingResult(true);
    try {
      const item = await api.historySave(r.item, r.files);
      markSaved({ ...r, item });
      await refreshHistory();
      showNotice(m.notice_result_saved(), true);
    } catch (e) {
      showNotice(m.notice_history_save_retry({ detail: String(e) }));
    } finally {
      setSavingResult(false);
    }
  };
  // UI and MCP share one admission path; the UI wrapper keeps the notice and
  // request-feedback behavior.
  const generate = async (count: number) => {
    if (storageBlocked() || !ready || importing) return;
    try {
      const result = await mcpActions.current.submitUi(count);
      const s = result.structured as { taskId?: string; workspace?: string };
      if (!s.taskId) return;
      setRequestFeedback({ key: s.workspace ?? workspaceKey(current.current), id: s.taskId });
    } catch (e) {
      // Not-accepted submissions stay silent, matching the old `if (!id)` path.
      if (
        e instanceof McpError &&
        (e.code === "UI_NOT_READY" || e.code === "UI_BUSY")
      )
        return;
      const detail =
        e instanceof McpError &&
        Array.isArray((e.details as { issues?: unknown })?.issues)
          ? (e.details as { issues: string[] }).issues.join("；")
          : null;
      showNotice(detail || m.notice_submit_failed({ detail: String(e) }));
    }
  };
  const useResult = async (
    im: WorkingImage,
    edit: boolean,
    source?: Draft,
  ): Promise<boolean> => {
    const d = edit ? ws.draftForIntent("edit") : current.current;
    const max = routeFor(d)?.maxRefs ?? 0;
    if (!edit && d.refs.length >= max) {
      showNotice(m.notice_refs_full({ count: max }));
      return false;
    }
    let keepReferences = true;
    const continuing =
      taskIntent(current.current) === "edit" &&
      source?.intent === "edit" &&
      primaryImage(source)?.uid === primaryImage(d)?.uid &&
      source.prompt === d.prompt;
    if (
      edit &&
      !continuing &&
      (d.refs.length || d.prompt.trim() || d.boxes.length || d.mask)
    ) {
      const decision = await new Promise<boolean | null>((decide) =>
        setEditHandoff({ draft: d, image: im, decide }),
      );
      setEditHandoff(null);
      if (decision == null) return false;
      if (ws.draftForIntent("edit") !== d) {
        showNotice(m.notice_edit_draft_changed());
        return false;
      }
      keepReferences = decision;
    }
    try {
      commit(
        edit
          ? beginImageEdit(d, im, keepReferences)
          : { ...d, refs: [...d.refs, { ...im, uid: crypto.randomUUID() }] },
        true,
      );
      setView("canvas");
      setTab("params");
      setSelected(null);
      return true;
    } catch (e) {
      showNotice(String(e));
      return false;
    }
  };
  const useGalleryRefs = async (ids: string[]) => {
    if (migrating.current) throw new Error(m.backend_storage_migrating());
    const target = current.current;
    const unique = [...new Set(ids)].filter(
      (id) => !target.refs.some((r) => r.assetId === id),
    );
    if (!unique.length) throw new Error(m.error_already_in_task());
    const room = Math.max(
      0,
      (routeFor(target)?.maxRefs ?? 0) - target.refs.length,
    );
    if (unique.length > room)
      throw new Error(m.error_ref_room({ count: room }));
    setImporting(true);
    try {
      const images = await Promise.all(
        unique.map(async (id) => ({
          ...(await api.galleryRead(id)),
          uid: crypto.randomUUID(),
        })),
      );
      if (workspaceKey(current.current) !== workspaceKey(target))
        throw new Error(m.error_task_switched());
      const next = {
        ...current.current,
        refs: [...current.current.refs, ...images],
      };
      commit(
        taskIntent(next) === "edit" && !next.baseId
          ? setPrimaryImage(next, next.refs[0].uid!)
          : next,
        true,
      );
      setGalleryPicker(false);
      setGalleryOpen(false);
      setTab("params");
      showNotice(m.notice_refs_added({ count: images.length }), true);
    } finally {
      setImporting(false);
    }
  };
  const editGalleryImage = async (item: GalleryItem) => {
    if (storageBlocked()) return false;
    setImporting(true);
    try {
      const image = await api.galleryRead(item.id);
      const accepted = await useResult(image, true);
      if (accepted) {
        setGalleryOpen(false);
        setGalleryPicker(false);
      }
      return accepted;
    } finally {
      setImporting(false);
    }
  };
  const restoreHistory = async (item: HistoryItem) => {
    if (storageBlocked()) return;
    if (importing) {
      showNotice(m.notice_wait_import());
      return;
    }
    try {
      const refs = await Promise.all(
        item.inputFiles.map(async (path, i) => ({
          ...(await api.importImage(path)),
          name: item.recipe?.refNames[i] ?? m.name_reference_n({ index: i + 1 }),
          uid: item.recipe?.refIds[i] ?? crypto.randomUUID(),
          purpose: item.recipe?.refPurposes?.[i],
          note: item.recipe?.refNotes?.[i],
        })),
      );
      const mask = item.maskFile
        ? {
            ...(await api.importImage(item.maskFile)),
            name: item.recipe?.maskName ?? m.name_mask(),
          }
        : null;
      if (!item.recipe) throw new Error(m.error_no_recipe());
      const raw = { ...item.recipe, refs, mask };
      commit(withIds(readDraft(raw)), true);
      setView("canvas");
      setTab("params");
      setSelected(null);
    } catch (e) {
      showNotice(m.notice_restore_failed({ detail: String(e) }));
    }
  };
  const historyAsInput = async (item: HistoryItem) => {
    if (storageBlocked()) return;
    try {
      if (!item.resultFiles[0]) throw new Error(m.error_no_result_image());
      await useResult(await api.importImage(item.resultFiles[0]), true);
    } catch (e) {
      setNotice(String(e));
    }
  };
  const chooseResult = async (
    index: number,
    target = result,
    preview = false,
  ) => {
    if (!target) return;
    const epoch = ++resultSelectionEpoch.current;
    try {
      const im = target.out.images[index];
      const size = await imageSize(im.dataUrl);
      if (epoch !== resultSelectionEpoch.current) return;
      // Decoding can finish after a later image in this batch arrives.
      const latest = currentResults.current[workspaceKey(target.snapshot)];
      if (latest?.item.id === target.item.id) target = latest;
      storeResult({
        ...target,
        selectedIndex: index,
        image: workingImageFromResult(target, index, size),
      });
      setView("result");
      setResultPreview(preview);
    } catch (e) {
      showNotice(m.notice_result_read_failed({ detail: String(e) }));
    }
  };
  const editResultImage = async (attempt: SavedResult, index: number) => {
    const im = attempt.out.images[index];
    if (!im?.dataUrl) return;
    try {
      const image = workingImageFromResult(
        attempt,
        index,
        await imageSize(im.dataUrl),
      );
      if ((await useResult(image, true, attempt.snapshot)) && resultPreview)
        setResultPreview(false);
    } catch (e) {
      showNotice(m.notice_result_read_failed({ detail: String(e) }));
    }
  };
  const saveResult = async () => {
    if (!result) return;
    try {
      const mime = result.out.images[result.selectedIndex].mediaType,
        ext = mime.includes("jpeg")
          ? "jpg"
          : mime.includes("webp")
            ? "webp"
            : "png";
      const path = await save({
        defaultPath: exportDefaultPath(
          prefs.saveDirectory,
          "lutriui-" +
            result.item.id.slice(0, 8) +
            (result.selectedIndex ? "-" + (result.selectedIndex + 1) : "") +
            "." +
            ext,
        ),
        filters: [{ name: m.file_image(), extensions: [ext] }],
      });
      if (path) {
        await api.saveDataUrl(result.image.dataUrl, path);
        showNotice(m.notice_saved_to({ path }), true);
      }
    } catch (e) {
      showNotice(m.notice_save_as_failed({ detail: String(e) }));
    }
  };
  const changeBoxes = (boxes: Box[]) =>
    commit(updateFluxBoxes(current.current, boxes));
  const reference = draft.refs.find((r) => r.uid === refPreview);
  const resultBase = getResultBase(result);
  const resultActions = (compact = false) =>
    result && (
      <ResultActions
        compact={compact}
        result={result}
        attempts={attempts[activeKey] ?? []}
        onAttempt={(r) => {
          setResults((s) => ({ ...s, [activeKey]: r }));
          setView("result");
        }}
        saving={savingResult}
        onSelect={(i) => void chooseResult(i)}
        onSave={() => void saveResult()}
        onCopy={() =>
          void api
            .copyImage(result.image.dataUrl)
            .then(() => showNotice(m.notice_image_copied(), true))
            .catch((e) => setNotice(String(e)))
        }
        onUse={async (image, edit) => {
          if ((await useResult(image, edit, result.snapshot)) && resultPreview)
            setResultPreview(false);
        }}
        onRetry={() => void retryHistory(result)}
      />
    );
  const references = (
    <ResizableReferences
      width={prefs.referenceSidebarWidth}
      onWidthChange={(referenceSidebarWidth) =>
        setPrefs((p) => ({ ...p, referenceSidebarWidth }))
      }
    >
      <References
        draft={draft}
        galleryIds={galleryIds}
        ready={ready}
        importing={importing || storageMigrating}
        onChange={(d, discrete = true) => commit(d, discrete)}
        onPreview={setRefPreview}
        onFiles={() => void openFiles()}
        onPaste={() => importBatch([api.clipboardImage], "clipboard")}
        onGallery={() => setGalleryPicker(true)}
        onUrl={() => setUrlDialog(true)}
        onNotice={setNotice}
      />
    </ResizableReferences>
  );
  const galleryProps = {
    revision: galleryRevision,
    importing: importing || storageMigrating,
    saveDirectory: prefs.saveDirectory,
    onRefresh: refreshHistory,
    onFiles: () => void openFiles(),
    onPaste: () => importBatch([api.clipboardImage], "clipboard"),
    onUse: useGalleryRefs,
    onEdit: editGalleryImage,
  };
  return (
    <div className={"workbench workbench-" + draft.family}>
      <header className="app-header">
        <div className="header-identity">
          <div className="brand">
            <span className="brand-mark" aria-hidden="true" />
            <strong>LutriUI</strong>
          </div>
          <div className="task-tabs segmented" aria-label={m.task_tabs()}>
            {(["create", "edit"] as const).map((task) => (
              <button
                key={task}
                aria-pressed={!galleryOpen && intent === task}
                className={!galleryOpen && intent === task ? "active" : ""}
                disabled={!ready || importing}
                onClick={() => {
                  setGalleryOpen(false);
                  setTab("params");
                  ws.switchIntent(task);
                }}
              >
                {task === "create" ? m.task_generate() : m.task_edit()}
              </button>
            ))}
            <button
              aria-pressed={galleryOpen}
              className={galleryOpen ? "active" : ""}
              disabled={importing}
              onClick={() => {
                setGalleryOpen(true);
                void refreshHistory();
              }}
            >
              {m.gallery()}
            </button>
          </div>
        </div>
        <div className="row">
          {generation.tasks.length > 0 && (
            <button onClick={() => setTasksOpen(true)}>
              {m.tasks_running({ count: generation.tasks.length })}
            </button>
          )}
          {generationTask && tab === "history" && (
            <button
              onClick={() => {
                ws.switchIntent(generationTask.intent ?? "create");
                setTab("params");
              }}
            >
              <span className="activity-dot" aria-hidden="true" />
              {m.tasks_view()}
            </button>
          )}
          <button
            disabled={!ready || importing}
            onClick={() => {
              ws.reset();
              setGalleryOpen(false);
              setTab("params");
              setSelected(null);
              setView("canvas");
            }}
          >
            <Plus size={15} />
            {m.action_new()}
          </button>
          <button
            disabled={galleryOpen || !undoStack.length || importing}
            onClick={() => {
              undo();
              setSelected(null);
            }}
            title={m.action_undo()}
          >
            <ArrowCounterClockwise size={16} />
          </button>
          <button
            disabled={galleryOpen || !redoStack.length || importing}
            onClick={() => {
              redo();
              setSelected(null);
            }}
            title={m.action_redo()}
          >
            <ArrowClockwise size={16} />
          </button>
          <button onClick={() => setSettings(true)}>
            <GearSix size={16} />
            {m.action_settings()}
          </button>
        </div>
      </header>
      <div className="gallery-page" hidden={!galleryOpen}>
        <Gallery {...galleryProps} />
      </div>
      <div
        className="workspace"
        style={galleryOpen ? { display: "none" } : undefined}
      >
        <ResizableSidebar
          widthPercent={prefs.sidebarWidthPercent}
          onWidthChange={(sidebarWidthPercent) =>
            setPrefs((p) => ({ ...p, sidebarWidthPercent }))
          }
        >
          <div className="tabs">
            <button
              className={tab === "params" ? "active" : ""}
              onClick={() => setTab("params")}
            >
              {m.scheme()}
            </button>
            <button
              className={tab === "history" ? "active" : ""}
              onClick={() => setTab("history")}
            >
              {m.history()}
            </button>
          </div>
          {tab === "params" ? (
            <WorkspaceSidebar
              onFamilyChange={ws.switchFamily}
              draft={draft}
              onChange={onChange}
              onReorder={(boxes) => commit({ ...current.current, boxes }, true)}
              selectedId={selectedId}
              onSelect={(id) => {
                setSelected(id);
                setView("canvas");
              }}
              onRename={(uid, id) => {
                commit(renameBox(current.current, uid, id), true);
                setSelected(id);
              }}
              providerStatus={pStatus}
              busy={
                generation.accepting ||
                !generation.ready ||
                !ready ||
                importing ||
                storageMigrating
              }
              generationTask={generationTask}
              onStopRemaining={() => generation.stopRemaining()}
              onCancelGeneration={() =>
                generationTask && generation.cancel(generationTask.id)
              }
              generationFailure={generation.failures[intent] ?? null}
              sentRequestId={
                requestFeedback?.key === activeKey
                  ? requestFeedback.id
                  : undefined
              }
              onShowTask={() =>
                generationTask &&
                ws.switchIntent(generationTask.intent ?? "create")
              }
              finalPreview={preview.finalPrompt}
              errors={errors}
              onGenerate={(count) => void generate(count)}
              onSettings={() => setSettings(true)}
              getVersion={ws.getVersion}
              applyOptimizedPrompt={ws.applyOptimizedPrompt}
            />
          ) : (
            <>
              {historyError && (
                <div className="history-error">
                  {historyError}
                  <button onClick={() => void refreshHistory()}>{m.action_retry()}</button>
                </div>
              )}
              <HistoryPanel
                saveDirectory={prefs.saveDirectory}
                items={history}
                onCancel={(id) => generation.cancel(id)}
                onRefresh={() => void refreshHistory()}
                onUseAsInput={(item) => void historyAsInput(item)}
                onRestoreEdit={(item) => void restoreHistory(item)}
              />
            </>
          )}
        </ResizableSidebar>
        {references}
        <main
          className="main-area"
          aria-label={m.workspace_label({
            name: familyById(draft.family).label,
          })}
        >
          <div className="canvas-toolbar">
            <div className="row">
              <strong>
                {intent === "create"
                  ? activeLayout && view === "canvas"
                    ? m.stage_compose()
                    : m.stage_preview()
                  : m.stage_edit()}
              </strong>
              {intent === "create" && activeLayout && result && (
                <button
                  onClick={() =>
                    setView(view === "canvas" ? "result" : "canvas")
                  }
                >
                  {view === "canvas" ? m.stage_view_result() : m.stage_back()}
                </button>
              )}
              {intent === "edit" && result && (
                <>
                  <button
                    className={view === "canvas" ? "active" : ""}
                    onClick={() => setView("canvas")}
                  >
                    {m.result_original()}
                  </button>
                  <button
                    className={view === "result" ? "active" : ""}
                    onClick={() => setView("result")}
                  >
                    {m.stage_edit_result()}
                  </button>
                  {resultBase && (
                    <button
                      className={view === "compare" ? "active" : ""}
                      onClick={() => setView("compare")}
                    >
                      {m.stage_compare()}
                    </button>
                  )}
                </>
              )}
            </div>
            {view === "canvas" && activeLayout && intent === "create" ? (
              <span className="muted">
                {draft.canvas.w}×{draft.canvas.h}
              </span>
            ) : !ordinaryGeneration && result && view !== "canvas" ? (
              <label className="check">
                <input
                  type="checkbox"
                  checked={original}
                  onChange={(e) => setOriginal(e.target.checked)}
                />
                {m.stage_full_size()}
              </label>
            ) : null}
            {ordinaryGeneration && previewAttempts.length > 0 && (
              <button
                aria-label={m.stage_close_preview()}
                title={
                  canClosePreview
                    ? m.stage_close_kept()
                    : m.stage_close_wait()
                }
                disabled={!canClosePreview}
                onClick={closeGenerationPreview}
              >
                <X size={16} />
                {m.stage_close()}
              </button>
            )}
          </div>
          {ordinaryGeneration ? (
            <div className="result-work-area">
              <GenerationGrid
                attempts={attempts[activeKey] ?? []}
                selected={result}
                onSelect={(r, i) => void chooseResult(i, r, true)}
                onEdit={(r, i) => void editResultImage(r, i)}
              />
            </div>
          ) : view === "canvas" &&
            ((intent === "edit" && !primaryImage(draft)) ||
              (intent === "create" && draft.family !== "flux")) ? (
            <div className="result-work-area">
              <div className="stage workspace-empty">
                <strong>
                  {intent === "edit"
                    ? m.stage_empty_edit()
                    : m.stage_empty_create()}
                </strong>
                <p className="muted">
                  {intent === "edit"
                    ? m.stage_empty_edit_help()
                    : m.stage_empty_create_help()}
                </p>
                {intent === "edit" && (
                  <button disabled={importing} onClick={() => void openFiles()}>
                    {m.stage_add_main()}
                  </button>
                )}
                {intent === "create" && result && (
                  <button onClick={() => setView("result")}>
                    {m.stage_view_result()}
                  </button>
                )}
              </div>
            </div>
          ) : view === "canvas" ? (
            <WorkspaceStage
              draft={draft}
              selectedId={selectedId}
              onSelect={setSelected}
              onChange={onChange}
              onBoxesChange={changeBoxes}
              onGestureStart={ws.beginGesture}
              onGestureEnd={ws.endGesture}
              onImportMask={() => void importMask()}
            />
          ) : (
            <div className="result-work-area">
              <ResultStage result={result} view={view} original={original} />
            </div>
          )}
          {!resultPreview && resultActions(ordinaryGeneration)}
        </main>
      </div>
      {resultPreview && result && (
        <Modal title={m.result_dialog()} large onClose={() => setResultPreview(false)}>
          <div className="gallery-preview">
            <img src={result.image.dataUrl} alt={m.result_alt()} />
          </div>
          {resultActions(true)}
        </Modal>
      )}
      {galleryPicker && (
        <Modal
          title={m.pick_from_gallery()}
          large
          onClose={() => {
            if (!importing) setGalleryPicker(false);
          }}
        >
          <Gallery
            {...galleryProps}
            picker={{
              limit: Math.max(
                0,
                (routeFor(draft)?.maxRefs ?? 0) - draft.refs.length,
              ),
              usedIds: draft.refs.flatMap((r) =>
                r.assetId ? [r.assetId] : [],
              ),
            }}
          />
        </Modal>
      )}
      {notice && (
        <div className="notice" role="status">
          <span>{notice}</span>
          <button onClick={() => setNotice("")} aria-label={m.notice_close()}>
            ×
          </button>
        </div>
      )}
      {storageMigrating && !settings && (
        <div className="notice" role="status">
          <span>{m.backend_storage_migrating()}</span>
        </div>
      )}
      {tasksOpen && (
        <Modal title={m.tasks_title()} onClose={() => setTasksOpen(false)}>
          <p className="help">{m.tasks_help()}</p>
          {generation.tasks.map((generationTask) => (
            <div className="queue-item" key={generationTask.id}>
              <strong>
                {m.tasks_active({
                  name: familyById(generationTask.family).label,
                })}
              </strong>
              <p>{generationTask.prompt}</p>
              {generationTask.stopRequested ? (
                <span>{m.tasks_stopped()}</span>
              ) : (
                <button
                  type="button"
                  onClick={() => generation.cancel(generationTask.id)}
                >
                  {m.action_cancel()}
                </button>
              )}
              <button
                onClick={() => {
                  ws.switchIntent(generationTask.intent ?? "create");
                  setTasksOpen(false);
                }}
              >
                {m.tasks_workspace()}
              </button>
            </div>
          ))}
          {!generation.tasks.length && <p>{m.tasks_empty()}</p>}
        </Modal>
      )}
      {settings && (
        <Settings
          prefs={prefs}
          initialProvider={draft.provider}
          onChange={setPrefs}
          status={pStatus}
          refresh={refreshProviders}
          onHistory={() => {
            setSettings(false);
            setTab("history");
          }}
          storageBusy={storageBusy}
          storageMigrating={storageMigrating}
          onMigrate={onMigrate}
          onMaintain={onMaintain}
          onClose={() => setSettings(false)}
        />
      )}
      {closeRequested && (
        <Modal title={m.close_title()} onClose={() => setCloseRequested(false)}>
          <p>{busy ? m.close_busy() : m.close_idle()}</p>
          <div className="row close-actions">
            <button
              className="primary"
              onClick={() => setCloseRequested(false)}
            >
              {busy ? m.close_wait() : m.close_back()}
            </button>
            <button onClick={() => void ws.closeApp()}>{m.close_anyway()}</button>
          </div>
        </Modal>
      )}
      {editHandoff && (
        <EditHandoff
          draft={editHandoff.draft}
          image={editHandoff.image}
          onDecision={editHandoff.decide}
        />
      )}
      {reference && (
        <Modal
          title={m.preview_ref({ name: reference.name })}
          large
          onClose={() => setRefPreview(null)}
        >
          <div className="image-preview">
            <img src={reference.dataUrl} alt={reference.name} />
          </div>
          <p className="help">
            {reference.width}×{reference.height} px
          </p>
        </Modal>
      )}
      {urlDialog && (
        <Modal title={m.url_title()} onClose={() => setUrlDialog(false)}>
          <label>
            {m.url_label()}
            <input
              type="url"
              placeholder="https://…"
              value={imageUrl}
              onChange={(e) => setImageUrl(e.target.value)}
            />
          </label>
          <p className="help">{m.url_help()}</p>
          <button
            className="primary"
            disabled={!imageUrl.trim()}
            onClick={() => {
              const url = imageUrl.trim();
              importBatch([() => api.importUrl(url)], "url");
              setUrlDialog(false);
              setImageUrl("");
            }}
          >
            {m.url_add()}
          </button>
        </Modal>
      )}
    </div>
  );
}
