import { m } from "../i18n";
import { cancelledError, generate } from "../lib/api";
import { downscaleDataUrl, imageSize } from "../lib/image";
import { buildRequest } from "../models";
import { fieldsFor } from "../models/catalog";
import { regionsEnabled } from "../models/flux/layout";
import { primaryImage, taskIntent } from "../lib/workspace";
import type {
  Draft,
  GenerateOutput,
  HistoryItem,
  WorkingImage,
  GenerationProgress,
} from "../lib/types";

export interface SavedResult {
  image: WorkingImage;
  selectedIndex: number;
  out: GenerateOutput;
  snapshot: Draft;
  item: HistoryItem;
  files: { kind: string; name: string; data: string }[];
  saved: boolean;
}

/** How a finished or in-flight job may touch the workspace result. */
export interface ResultPublish {
  /** This job is not the foreground submission. Settle its own attempt only. */
  settleOnly?: boolean;
  /** Do not raise a completion notice. Cancel already told the user. */
  quiet?: boolean;
}

/**
 * The latest submission of an intent owns the workspace result.
 * A job that no longer owns it only settles an attempt already on screen.
 * Cancel keeps that update, but does not raise a completion notice.
 */
export function resultPublishFor(
  jobId: string,
  foregroundJobId: string | undefined,
  cancelled: boolean,
): ResultPublish | undefined {
  const owns = foregroundJobId === undefined || foregroundJobId === jobId;
  if (!owns) return { settleOnly: true, quiet: true };
  if (cancelled) return { quiet: true };
  return undefined;
}
export function getResultBase(result: SavedResult | null) {
  return result ? primaryImage(result.snapshot) : null;
}
/** Gallery-backed candidate for Edit; size is decoded by the caller. */
export function workingImageFromResult(
  result: SavedResult,
  index: number,
  size: { width: number; height: number },
): WorkingImage {
  return {
    uid: crypto.randomUUID(),
    dataUrl: result.out.images[index].dataUrl,
    ...size,
    name: m.name_result_n({ index: index + 1 }),
    assetId: result.item.resultAssetIds?.[index],
  };
}
/** Persist the immutable recipe and original inputs before any provider call. */
export function queuedGeneration(snapshot: Draft) {
  if (fieldsFor(snapshot).seed && snapshot.params.seed == null) {
    const seed = crypto.getRandomValues(new Uint32Array(1))[0] & 0x7fffffff;
    snapshot = { ...snapshot, params: { ...snapshot.params, seed } };
  }
  const { refs, mask, ...recipe } = snapshot;
  const request = buildRequest(
    snapshot,
    snapshot.refs.map((r) => r.dataUrl),
    mask?.dataUrl,
  );
  const item: HistoryItem = {
    id: crypto.randomUUID(),
    createdAt: Date.now(),
    provider: snapshot.provider,
    model: request.model,
    mode: taskIntent(snapshot) === "edit" ? "edit" : "t2i",
    prompt: snapshot.prompt,
    finalPrompt: request.finalPrompt,
    params: { ...request.params },
    boxes: regionsEnabled(snapshot) ? snapshot.boxes : [],
    canvasWidth: snapshot.canvas.w,
    canvasHeight: snapshot.canvas.h,
    inputFiles: [],
    resultFiles: [],
    maskFile: null,
    thumb: null,
    usage: null,
    cost: null,
    status: "queued",
    error: null,
    recipe: {
      ...recipe,
      refNames: refs.map((r) => r.name),
      refIds: refs.map((r) => r.uid!),
      refPurposes: refs.map((r) => r.purpose),
      refNotes: refs.map((r) => r.note),
      maskName: mask?.name,
    },
  };
  const files = refs.map((r, i) => ({
    kind: "input",
    name: "input_" + i,
    data: r.dataUrl,
  }));
  if (mask) files.push({ kind: "mask", name: "mask", data: mask.dataUrl });
  return { snapshot, item, files };
}
/** One immutable snapshot, one paid submission, all output images retained. */
export async function executeGeneration(
  snapshot: Draft,
  onProgress?: (progress: GenerationProgress) => void,
  queued?: HistoryItem,
  requestId?: string,
  beforeSubmit?: () => Promise<void>,
  signal?: AbortSignal,
): Promise<SavedResult> {
  onProgress?.({ phase: "preparing" });
  const images = await Promise.all(
    snapshot.refs.map((r) =>
      snapshot.compressEnabled
        ? downscaleDataUrl(r.dataUrl, snapshot.maxInputEdge)
        : Promise.resolve(r.dataUrl),
    ),
  );
  const mask = snapshot.mask
    ? snapshot.compressEnabled
      ? await downscaleDataUrl(snapshot.mask.dataUrl, snapshot.maxInputEdge)
      : snapshot.mask.dataUrl
    : undefined;
  onProgress?.({ phase: "waiting" });
  const request = buildRequest(snapshot, images, mask);
  if (queued) {
    request.requestId = requestId ?? queued.id;
    request.historyId = queued.id;
  }
  await beforeSubmit?.();
  if (signal?.aborted) throw cancelledError();
  const out = await generate(request, onProgress, signal);
  if (signal?.aborted) throw cancelledError();
  if (!out.images[0]) throw new Error(m.error_no_image());
  const image: WorkingImage = {
    uid: crypto.randomUUID(),
    dataUrl: out.images[0].dataUrl,
    ...(await imageSize(out.images[0].dataUrl)),
    name: m.name_result(),
  };
  const { refs, mask: originalMask, ...recipe } = snapshot;
  const item: HistoryItem = {
    id: queued?.id ?? crypto.randomUUID(),
    createdAt: queued?.createdAt ?? Date.now(),
    provider: out.provider,
    model: out.model,
    mode: taskIntent(snapshot) === "edit" ? "edit" : "t2i",
    prompt: snapshot.prompt,
    finalPrompt: out.finalPrompt,
    params: { ...request.params },
    boxes: regionsEnabled(snapshot) ? snapshot.boxes : [],
    canvasWidth: snapshot.canvas.w,
    canvasHeight: snapshot.canvas.h,
    inputFiles: [],
    resultFiles: [],
    maskFile: null,
    thumb: null,
    resultDetails: resultDetails(out.images),
    usage: out.usage,
    cost: out.usage?.cost ?? null,
    status: "ok",
    recipe: {
      ...recipe,
      refNames: refs.map((r) => r.name),
      refIds: refs.map((r) => r.uid!),
      refPurposes: refs.map((r) => r.purpose),
      refNotes: refs.map((r) => r.note),
      maskName: originalMask?.name,
    },
  };
  const files = [
    ...refs.map((r, i) => ({
      kind: "input",
      name: "input_" + i,
      data: r.dataUrl,
    })),
    ...out.images.map((im, i) => ({
      kind: "result",
      name: "result_" + i,
      data: im.dataUrl,
    })),
    ...(originalMask
      ? [{ kind: "mask", name: "mask", data: originalMask.dataUrl }]
      : []),
  ];
  return { image, selectedIndex: 0, out, snapshot, item, files, saved: false };
}

/** Merge outputs without duplicating the common prompt, inputs or mask. */
export function mergeGenerationResults(
  previous: SavedResult | null,
  next: SavedResult,
): SavedResult {
  if (!previous) return next;
  const images = [...previous.out.images, ...next.out.images];
  const usage = { ...previous.out.usage, ...next.out.usage };
  for (const key of ["cost", "credits", "total_tokens"]) {
    const a = previous.out.usage?.[key],
      b = next.out.usage?.[key];
    if (typeof a === "number" && typeof b === "number") usage[key] = a + b;
    else delete usage[key];
  }
  return {
    ...previous,
    out: {
      ...previous.out,
      images,
      usage,
      notes: [...new Set([...previous.out.notes, ...next.out.notes])],
    },
    item: {
      ...previous.item,
      resultDetails: resultDetails(images),
      usage,
      cost: usage.cost ?? null,
    },
    files: [
      ...previous.files.filter((f) => f.kind !== "result"),
      ...images.map((im, i) => ({
        kind: "result",
        name: "result_" + i,
        data: im.dataUrl,
      })),
    ],
    saved: false,
  };
}

function resultDetails(images: GenerateOutput["images"]) {
  return Object.fromEntries(
    images.flatMap((image, i) =>
      image.details ? [[`result_${i}`, image.details]] : [],
    ),
  );
}
