import { useEffect, useRef, useState, type RefObject } from "react";
import { localizeStored, m } from "../i18n";
import * as api from "../lib/api";
import { readDraft, taskIntent } from "../lib/workspace";
import { singleImageDraft } from "../models/catalog";
import { makeThumb } from "../lib/image";
import {
  executeGeneration,
  queuedGeneration,
  mergeGenerationResults,
  type SavedResult,
} from "./generation";
import type { Draft, GenerationTask } from "../lib/types";

type Job = ReturnType<typeof queuedGeneration> & {
  snapshots?: Draft[];
  stopRequested?: boolean;
  cancelled?: boolean;
  abort?: AbortController;
  lastErrorCode?: string;
};
export function useGenerationTasks(
  active: RefObject<boolean>,
  onResult: (result: SavedResult) => void,
  onHistory: () => Promise<void>,
  onNotice: (message: string) => void,
) {
  const [tasks, setTasks] = useState<
    (GenerationTask & { id: string; prompt: string })[]
  >([]);
  const [accepting, setAccepting] = useState(false);
  const [ready, setReady] = useState(false);
  const [failure, setFailure] = useState<{
    intent: ReturnType<typeof taskIntent>;
    message: string;
    code?: string;
  } | null>(null);
  const running = useRef(new Map<string, Job>());
  const progress = useRef(new Map<string, GenerationTask>());
  const locked = useRef(false);
  const alive = useRef(true);
  const restoreEpoch = useRef(0);
  const callbacks = useRef({ onResult, onHistory, onNotice });
  callbacks.current = { onResult, onHistory, onNotice };
  const sync = () => {
    active.current = running.current.size > 0 || locked.current;
    if (alive.current)
      setTasks(
        Array.from(progress.current, ([id, task]) => ({
          ...task,
          id,
          prompt: running.current.get(id)?.snapshot.prompt ?? "",
        })),
      );
  };
  const updateTask = (
    id: string,
    update: (task: GenerationTask | null) => GenerationTask | null,
  ) => {
    const task = update(progress.current.get(id) ?? null);
    if (task) progress.current.set(id, task);
    else progress.current.delete(id);
    sync();
  };
  const refresh = () =>
    callbacks.current
      .onHistory()
      .catch((e) =>
        callbacks.current.onNotice(
          m.notice_history_refresh_failed({ detail: String(e) }),
        ),
      );
  const run = async (job: Job) => {
    if (!alive.current || running.current.has(job.item.id)) return;
    running.current.set(job.item.id, job);
    const snapshots = job.snapshots ?? [job.snapshot];
    const requests = job.item.batch?.requests;
    updateTask(job.item.id, () => ({
      family: job.snapshot.family,
      intent: taskIntent(job.snapshot),
      modelId: job.snapshot.modelId,
      provider: job.snapshot.provider,
      startedAt: Date.now(),
      total: snapshots.length,
      phase: "preparing",
      remainingRequests: snapshots.length,
    }));
    let combined: SavedResult | null = null;
    let completed = 0;
    let persistenceFailed = false;
    // Serialize writes to this history record, while provider requests run concurrently.
    let writes: Promise<void> = Promise.resolve();
    const persist = (update?: () => void) => {
      const next = writes.then(async () => {
        update?.();
        const saved = await api.historySave(job.item, job.files);
        job.item = {
          ...saved,
          batch: job.item.batch,
          error: job.item.error,
          phase: job.item.phase,
          taskId: job.item.taskId,
          mcpSubmission: job.item.mcpSubmission,
        };
      });
      writes = next.catch(() => {
        persistenceFailed = true;
      });
      return next;
    };
    const errorMessage = (e: unknown) =>
      (e instanceof Error ? e.message : String(e)) +
      (e instanceof api.AppError && e.hint ? " · " + e.hint : "");
    const errorCode = (e: unknown) =>
      e instanceof api.AppError ? e.code : undefined;
    try {
      job.item = { ...job.item, status: "running", phase: "preparing" };
      await persist();
      await refresh();
      await Promise.all(
        snapshots.map(async (snapshot, i) => {
          try {
            const result = await executeGeneration(
              snapshot,
              (event) => {
                if (job.cancelled) return;
                const changed =
                  job.item.phase !== event.phase ||
                  (!!event.taskId && job.item.taskId !== event.taskId);
                job.item = {
                  ...job.item,
                  phase: event.phase,
                  taskId: event.taskId ?? job.item.taskId,
                };
                if (requests && event.taskId) requests[i].taskId = event.taskId;
                if (alive.current)
                  updateTask(job.item.id, (t) =>
                    t ? { ...t, ...event, completed } : t,
                  );
                if (changed) void refresh();
              },
              job.item,
              requests?.[i].requestId,
              async () => {
                await persist(() => {
                  if (
                    job.cancelled ||
                    job.stopRequested ||
                    persistenceFailed ||
                    !alive.current
                  ) {
                    if (requests) {
                      requests[i].status = "skipped";
                      requests[i].error = job.cancelled
                        ? m.error_user_cancelled()
                        : m.error_request_unsent();
                    }
                    throw job.cancelled
                      ? api.cancelledError()
                      : new Error(m.error_request_unsent());
                  }
                  if (requests) requests[i].status = "running";
                });
                if (alive.current)
                  updateTask(job.item.id, (t) =>
                    t
                      ? {
                          ...t,
                          remainingRequests:
                            requests?.filter((r) => r.status === "queued")
                              .length ?? 0,
                        }
                      : t,
                  );
              },
              job.abort?.signal,
            );
            const thumb = await makeThumb(result.image.dataUrl).catch(
              () => null,
            );
            if (job.cancelled) throw api.cancelledError();
            await persist(() => {
              if (requests) requests[i].status = "ok";
              combined = mergeGenerationResults(combined, result);
              job.item = {
                ...combined.item,
                batch: job.item.batch,
                status: "running",
                error: job.item.error,
                taskId: job.item.taskId,
                phase: "saving",
                thumb: job.item.thumb ?? thumb,
                mcpSubmission: job.item.mcpSubmission,
              };
              job.files = combined.files;
            });
            if (job.cancelled) throw api.cancelledError();
            if (combined && alive.current)
              callbacks.current.onResult({
                ...combined,
                item: job.item,
                saved: true,
              });
          } catch (e) {
            const cancelled = api.isCancelledError(e) || job.cancelled;
            if (cancelled) {
              if (
                requests &&
                requests[i].status !== "ok" &&
                requests[i].status !== "skipped"
              ) {
                requests[i].status = "skipped";
                requests[i].error = m.error_user_cancelled();
              }
            } else {
              const message = errorMessage(e);
              job.lastErrorCode = errorCode(e) ?? job.lastErrorCode;
              // A result whose history write failed remains available for manual saving.
              if (
                requests?.[i].status !== "skipped" &&
                requests?.[i].status !== "ok"
              ) {
                if (requests) {
                  requests[i].status = "failed";
                  requests[i].error = message;
                }
              }
              if (requests?.[i].status !== "skipped")
                job.item.error = [
                  job.item.error && localizeStored(job.item.error),
                  m.error_request_failed({ index: i + 1, detail: message }),
                ]
                  .filter(Boolean)
                  .join("；");
              if (combined && alive.current)
                callbacks.current.onResult({
                  ...combined,
                  item: job.item,
                  saved: false,
                });
            }
          } finally {
            completed++;
            if (alive.current)
              updateTask(job.item.id, (t) => (t ? { ...t, completed } : t));
            await refresh();
          }
        }),
      );
    } catch (e) {
      job.item.error = m.error_history_save_stopped({
        detail: errorMessage(e),
      });
      for (const request of requests ?? []) {
        if (request.status === "queued") {
          request.status = "skipped";
          request.error = m.error_history_save_skipped();
        }
      }
    }
    // All request handlers have settled before the final aggregate is saved.
    const result = combined as SavedResult | null;
    job.item = {
      ...job.item,
      status: result
        ? job.item.error || job.stopRequested || job.cancelled
          ? "partial"
          : "ok"
        : job.cancelled
          ? "cancelled"
          : "failed",
      phase: "completed",
    };
    let saved = false;
    try {
      await persist();
      saved = true;
    } catch (e) {
      callbacks.current.onNotice(
        m.error_history_resave({ detail: errorMessage(e) }),
      );
    }
    if (result && alive.current)
      callbacks.current.onResult({ ...result, item: job.item, saved });
    else if (alive.current && !job.cancelled) {
      const message = localizeStored(job.item.error ?? "");
      setFailure({
        intent: taskIntent(job.snapshot),
        message,
        code: job.lastErrorCode,
      });
      callbacks.current.onNotice(
        m.notice_task_failed({
          detail: message,
        }),
      );
    }
    await refresh();
    running.current.delete(job.item.id);
    updateTask(job.item.id, () => null);
  };
  const submit = async (
    snapshot: Draft,
    count = 1,
    mcpSubmission?: { idempotencyKey: string; workspaceVersion: string },
  ) => {
    if (locked.current || !ready) return;
    setFailure(null);
    if (!Number.isInteger(count) || count < 1 || count > 20)
      throw new Error(m.error_count_range());
    locked.current = true;
    setAccepting(true);
    sync();
    try {
      const entries = Array.from({ length: count }, () =>
        queuedGeneration(singleImageDraft(structuredClone(snapshot))),
      );
      const job: Job = {
        ...entries[0],
        snapshots: entries.map((entry) => entry.snapshot),
        abort: new AbortController(),
      };
      // MCP submissions reserve the idempotency key as the task id so the
      // durable ledger can never bill the same submission twice.
      if (mcpSubmission)
        job.item = {
          ...job.item,
          id: mcpSubmission.idempotencyKey,
          mcpSubmission,
        };
      job.item.batch = {
        requests: entries.map((entry) => ({
          requestId: entry.item.id,
          seed: entry.snapshot.params.seed,
          status: "queued",
        })),
      };
      await api.historySave(job.item, job.files);
      // Each submission starts independently after its snapshot is persisted.
      void run(job);
      return job.item.id;
    } finally {
      locked.current = false;
      setAccepting(false);
      sync();
    }
  };
  const stopRemaining = (id?: string): { stopped: boolean } => {
    const job = id
      ? running.current.get(id)
      : running.current.values().next().value;
    if (
      !job ||
      job.stopRequested ||
      !job.item.batch?.requests.some((r) => r.status === "queued")
    )
      return { stopped: false };
    job.stopRequested = true;
    job.item.batch.stopped = true;
    for (const request of job.item.batch.requests) {
      if (request.status === "queued") {
        request.status = "skipped";
        request.error = m.error_user_stopped();
      }
    }
    updateTask(job.item.id, (t) =>
      t ? { ...t, remainingRequests: 0, stopRequested: true } : t,
    );
    callbacks.current.onNotice(m.notice_stopped_remaining());
    return { stopped: true };
  };
  const skipQueued = (job: Job) => {
    if (!job.item.batch?.requests) return;
    job.item.batch.stopped = true;
    for (const request of job.item.batch.requests) {
      if (request.status === "queued") {
        request.status = "skipped";
        request.error = m.error_user_cancelled();
      }
    }
  };
  const cancel = (id?: string): { cancelled: boolean } => {
    const job = id
      ? running.current.get(id)
      : running.current.values().next().value;
    if (!job || job.cancelled) return { cancelled: false };
    job.cancelled = true;
    job.abort?.abort();
    skipQueued(job);
    job.stopRequested = true;
    setFailure(null);
    updateTask(job.item.id, () => null);
    callbacks.current.onNotice(m.notice_task_cancelled());
    return { cancelled: true };
  };
  useEffect(() => {
    alive.current = true;
    const epoch = ++restoreEpoch.current;
    const isCurrent = () => alive.current && restoreEpoch.current === epoch;
    const restore = async () => {
      try {
        const restored: Job[] = [];
        for (const item of (await api.historyList())
          .filter((it) => it.status === "queued")
          .sort((a, b) => a.createdAt - b.createdAt)) {
          if (!isCurrent()) return;
          try {
            if (!item.recipe) throw new Error(m.error_missing_recipe());
            const refs = await Promise.all(
              item.inputFiles.map(async (path, i) => ({
                ...(await api.importImage(path)),
                name: item.recipe!.refNames[i],
                uid: item.recipe!.refIds[i],
                purpose: item.recipe!.refPurposes?.[i],
                note: item.recipe!.refNotes?.[i],
              })),
            );
            const mask = item.maskFile
              ? {
                  ...(await api.importImage(item.maskFile)),
                  name: item.recipe.maskName ?? m.name_mask(),
                }
              : null;
            const snapshot = readDraft({ ...item.recipe, refs, mask });
            const job: Job = {
              ...queuedGeneration(snapshot),
              abort: new AbortController(),
            };
            if (item.batch)
              job.snapshots = item.batch.requests.map((request) => ({
                ...snapshot,
                params: { ...snapshot.params, seed: request.seed },
              }));
            if (!isCurrent()) return;
            job.item = {
              ...item,
              inputFiles: [],
              resultFiles: [],
              maskFile: null,
            };
            restored.push(job);
          } catch (e) {
            await api.historySave(
              {
                ...item,
                status: "failed",
                error: m.error_restore_failed({ detail: String(e) }),
              },
              [],
            );
          }
        }
        sync();
        if (isCurrent()) {
          setReady(true);
          for (const job of restored) void run(job);
        }
      } catch (e) {
        callbacks.current.onNotice(
          m.notice_task_read_failed({ detail: String(e) }),
        );
      }
    };
    void restore();
    return () => {
      alive.current = false;
      restoreEpoch.current++;
    };
  }, []);
  return {
    task: tasks[0] ?? null,
    tasks,
    accepting,
    ready,
    submit,
    stopRemaining,
    cancel,
    failure,
  };
}
