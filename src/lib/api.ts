// Tauri 后端命令的类型化封装与错误解析。

import { invoke, convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { localizeMessage, localizeStored, m } from "../i18n";
import { listen } from "@tauri-apps/api/event";
import type { GenerationProgress } from "./types";
import type {
  GenerateOutput,
  GenerateRequestPayload,
  HistoryItem,
  ProviderStatus,
} from "./types";
import type {
  Draft,
  WorkspaceSession,
  ProviderId,
  CredentialSettings,
  Board,
  GalleryItem,
  GalleryQuery,
  GalleryPage,
  GalleryFacets,
  GalleryPatch,
  GalleryBatchResult,
  LibraryStats,
  LibraryMaintainOp,
  MaintenanceReport,
} from "./types";

export const CANCELLED_CODES = [
  "backend_llm_cancelled",
  "backend_generation_cancelled",
] as const;

export class AppError extends Error {
  status?: number;
  hint?: string;
  code?: string;

  constructor(
    message: string,
    opts?: { status?: number; hint?: string; code?: string },
  ) {
    super(message);
    this.name = "AppError";
    this.status = opts?.status;
    this.hint = opts?.hint;
    this.code = opts?.code;
  }

  // 字符串化时只输出已本地化的说明，避免界面出现 "AppError: " 前缀。
  toString() {
    return this.message;
  }
}

export function cancelledError(
  code: (typeof CANCELLED_CODES)[number] = "backend_generation_cancelled",
) {
  return new AppError(localizeMessage(code, "Cancelled"), { code });
}

export function isCancelledError(e: unknown): boolean {
  if (!e || typeof e !== "object") return false;
  if ("name" in e && (e as { name?: string }).name === "AbortError") return true;
  return (
    e instanceof AppError &&
    !!e.code &&
    (CANCELLED_CODES as readonly string[]).includes(e.code)
  );
}

/** 后端错误为 ProviderError 的 JSON 字符串，解析为可读错误 */
function parseBackendError(e: unknown): AppError {
  const raw =
    typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  try {
    const j = JSON.parse(raw) as {
      status?: number;
      message?: string;
      hint?: string;
      code?: string;
      hint_code?: string;
      params?: Record<string, string | number>;
    };
    if (j && typeof j.message === "string") {
      const params = j.params ?? {};
      return new AppError(localizeMessage(j.code, j.message, params), {
        status: j.status,
        code: j.code,
        hint:
          j.hint_code != null
            ? localizeMessage(j.hint_code, j.hint ?? "", params)
            : j.hint,
      });
    }
  } catch {
    // 不是 JSON，按纯文本处理
  }
  return new AppError(raw);
}

/** 所有命令经过此入口，后端错误码统一按当前语言展示。 */
async function call<T>(command: string, args?: Record<string, unknown>) {
  try {
    return await invoke<T>(command, args);
  } catch (e) {
    throw parseBackendError(e);
  }
}

// 默认名称由后端留空（不保存本地化文案），在进入界面时按当前语言补全。
function withDefaultName(image: ImportedImage, name: () => string) {
  return image.name ? image : { ...image, name: name() };
}

export async function providerStatus(): Promise<ProviderStatus> {
  if (!isTauri())
    return {
      openrouter: false,
      bfl: false,
      comfy: false,
      runware: false,
      google: false,
      ark: false,
      byteplus: false,
      xai: false,
      qwencloud: false,
    };
  return call<ProviderStatus>("provider_status");
}
export const isDesktop = () => isTauri();

export async function generate(
  request: GenerateRequestPayload,
  onProgress?: (progress: GenerationProgress) => void,
  signal?: AbortSignal,
): Promise<GenerateOutput> {
  let off: (() => void) | undefined;
  const requestId = request.requestId;
  const abort = () => {
    if (requestId) void call("generate_cancel", { requestId });
  };
  if (signal?.aborted) {
    abort();
    throw cancelledError();
  }
  if (signal && requestId) signal.addEventListener("abort", abort, { once: true });
  try {
    if (onProgress)
      off = await listen<GenerationProgress>("generation-progress", (event) => {
        if (event.payload.requestId === request.requestId)
          onProgress(event.payload);
      });
    const out = await call<GenerateOutput>("generate", { request });
    if (signal?.aborted) throw cancelledError();
    return out;
  } finally {
    signal?.removeEventListener("abort", abort);
    off?.();
  }
}

export interface ImportedImage {
  assetId?: string;
  dataUrl: string;
  width: number;
  height: number;
  name: string;
}

export function importImage(path: string): Promise<ImportedImage> {
  return call<ImportedImage>("import_image", { path });
}

export function saveDataUrl(dataUrl: string, path: string): Promise<string> {
  return call<string>("save_data_url", { dataUrl, path });
}

export function historyList(): Promise<HistoryItem[]> {
  if (!isTauri()) return Promise.resolve([]);
  return call<HistoryItem[]>("history_list");
}

export function historySave(
  item: HistoryItem,
  files: { kind: string; name: string; data: string }[],
): Promise<HistoryItem> {
  return call<HistoryItem>("history_save", { item, files });
}

export function historyDelete(id: string): Promise<void> {
  return call<void>("history_delete", { id });
}

/** 历史图片绝对路径 → asset 协议 URL */
export function assetUrl(absPath: string): string {
  return convertFileSrc(absPath);
}

export const draftLoad = () => call<unknown>("draft_load");
export const draftSave = (draft: Draft | WorkspaceSession) =>
  call<void>("draft_save", { draft });
export const credentialSave = (provider: ProviderId, key: string) =>
  call<void>("credential_save", { provider, key });
export const credentialRemove = (provider: ProviderId) =>
  call<void>("credential_remove", { provider });
export const credentialCheck = async (provider: ProviderId) =>
  localizeStored(await call<string>("credential_check", { provider }));
export const credentialConfigure = (
  provider: ProviderId,
  settings: CredentialSettings,
) => call<void>("credential_configure", { provider, ...settings });

export interface LlmStatus {
  baseUrl: string;
  model: string;
  hasKey: boolean;
  configured: boolean;
}

export const llmStatus = () => call<LlmStatus>("llm_status");
export const llmConfigure = (baseUrl: string, model: string) =>
  call<void>("llm_configure", { baseUrl, model });
export const llmCredentialSave = (key: string) =>
  call<void>("llm_credential_save", { key });
export const llmCredentialRemove = () => call<void>("llm_credential_remove");
export const llmCheck = async () =>
  localizeStored(await call<string>("llm_check"));
export async function promptOptimize(
  family: string,
  userContent: string,
  requestId: string,
  signal?: AbortSignal,
) {
  const abort = () => {
    void call("prompt_optimize_cancel", { requestId });
  };
  if (signal?.aborted) {
    abort();
    throw cancelledError("backend_llm_cancelled");
  }
  if (signal) signal.addEventListener("abort", abort, { once: true });
  try {
    const text = await call<string>("prompt_optimize", {
      family,
      userContent,
      requestId,
    });
    if (signal?.aborted) throw cancelledError("backend_llm_cancelled");
    return text;
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}
export const importUrl = async (url: string) =>
  withDefaultName(await call<ImportedImage>("import_url", { url }), () =>
    m.default_name_web(),
  );
export const clipboardImage = async () =>
  withDefaultName(await call<ImportedImage>("clipboard_image"), () =>
    m.default_name_clipboard(),
  );
export const copyImage = (dataUrl: string) =>
  call<void>("copy_image", { dataUrl });
export const historyStorage = () =>
  isTauri()
    ? call<string>("history_storage")
    : Promise.resolve(m.error_desktop_only_history());

export interface LibraryMigration {
  previousPath: string;
  path: string;
}

export const libraryMigrate = (path: string) =>
  call<LibraryMigration>("library_migrate", { path });

function withDefaultItemName(item: GalleryItem): GalleryItem {
  return {
    ...item,
    name: item.name || m.default_name_generated(),
    boardIds: item.boardIds ?? [],
  };
}

function localizedBatch(result: GalleryBatchResult): GalleryBatchResult {
  return {
    ...result,
    failed: result.failed.map((failure) => ({
      ...failure,
      message: String(parseBackendError(failure.message)),
    })),
  };
}

export const galleryQuery = async (query: GalleryQuery): Promise<GalleryPage> => {
  const page = await call<GalleryPage>("gallery_query", { query });
  return { ...page, items: page.items.map(withDefaultItemName) };
};
export const galleryFacets = () => call<GalleryFacets>("gallery_facets");
export const galleryGet = async (ids: string[]): Promise<GalleryItem[]> => {
  const items = await call<GalleryItem[]>("gallery_get", { ids });
  return items.map(withDefaultItemName);
};
export const galleryPatch = (ids: string[], patch: GalleryPatch) =>
  call<GalleryItem[]>("gallery_patch", { ids, patch });
export const galleryDelete = async (ids: string[]) =>
  localizedBatch(await call<GalleryBatchResult>("gallery_delete", { ids }));
export const galleryExport = async (ids: string[], directory: string) =>
  localizedBatch(
    await call<GalleryBatchResult>("gallery_export", { ids, directory }),
  );
export const galleryImport = (
  image: ImportedImage,
  source: "file" | "clipboard" | "url",
) =>
  call<GalleryItem>("gallery_import", {
    dataUrl: image.dataUrl,
    name: image.name,
    source,
  }).then(withDefaultItemName);
export const boardList = () => call<Board[]>("board_list");
export const boardCreate = (name: string) => call<Board>("board_create", { name });
export const boardRename = (id: string, name: string) =>
  call<Board>("board_rename", { id, name });
export const boardDelete = (id: string) => call<void>("board_delete", { id });
export const boardAdd = async (boardId: string, ids: string[]) =>
  localizedBatch(await call<GalleryBatchResult>("board_add", { boardId, ids }));
export const boardRemove = async (boardId: string, ids: string[]) =>
  localizedBatch(
    await call<GalleryBatchResult>("board_remove", { boardId, ids }),
  );
export const boardMove = async (
  fromBoardId: string,
  toBoardId: string,
  ids: string[],
) =>
  localizedBatch(
    await call<GalleryBatchResult>("board_move", {
      fromBoardId,
      toBoardId,
      ids,
    }),
  );
export const galleryRead = async (id: string): Promise<ImportedImage> => ({
  ...withDefaultName(
    await call<ImportedImage>("gallery_read", { id }),
    () => m.default_name_generated(),
  ),
  assetId: id,
});
/** lutri-thumb 协议缩略图；revision 变化时强制重新读取。 */
export function galleryThumbnail(id: string, revision?: number): string {
  const url = convertFileSrc(id, "lutri-thumb");
  return revision != null ? `${url}?r=${revision}` : url;
}
// MCP server control + the WebView bridge it uses to reach the live workspace.
export interface McpStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  url: string | null;
  hasToken: boolean;
  uiReady: boolean;
  error: string | null;
}
export interface McpConnection {
  url: string;
  token: string;
}
export interface McpSubmissionRecord {
  idempotencyKey: string;
  workspaceVersion: string;
  taskId: string;
}
export interface McpConfigureResult extends McpStatus {
  /** Set only when this enable minted a token the settings page should show. */
  issuedToken?: string;
}
export const mcpStatus = () => call<McpStatus>("mcp_status");
export const mcpConfigure = (enabled: boolean, port: number) =>
  call<McpConfigureResult>("mcp_configure", { enabled, port });
export const mcpConnection = () => call<McpConnection>("mcp_connection");
export const mcpRotateToken = () => call<McpStatus>("mcp_rotate_token");
export const mcpSubmissionGet = (idempotencyKey: string) =>
  call<McpSubmissionRecord | null>("mcp_submission_get", { idempotencyKey });
export const mcpBridgeRegister = (instanceId: string) =>
  call<void>("mcp_bridge_register", { instanceId });
export const mcpBridgeUnregister = (instanceId: string) =>
  call<void>("mcp_bridge_unregister", { instanceId });
export const mcpBridgeClaim = (requestId: string, instanceId: string) =>
  call<boolean>("mcp_bridge_claim", { requestId, instanceId });
export const mcpBridgeReply = (
  requestId: string,
  instanceId: string,
  result?: unknown,
  error?: { code: string; message: string; details?: unknown },
) => call<void>("mcp_bridge_reply", { requestId, instanceId, result, error });
export interface McpRequestEvent {
  requestId: string;
  instanceId: string;
  operation: string;
  args: unknown;
  deadlineMs: number;
}

export const libraryStats = () => call<LibraryStats>("library_stats");
export const libraryMaintain = async (operation: LibraryMaintainOp) => {
  const report = await call<MaintenanceReport>("library_maintain", {
    operation,
  });
  return {
    ...report,
    failed: report.failed.map((failure) => ({
      ...failure,
      message: String(parseBackendError(failure.message)),
    })),
  };
};
