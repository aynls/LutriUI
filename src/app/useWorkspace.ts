import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "../lib/api";
import { DEFAULT_PARAMS } from "../lib/params";
import {
  DEFAULT_PREFERENCES,
  newDraft,
  withIds,
  readSession,
  changeFamily,
  taskIntent,
  workspaceKey,
} from "../lib/workspace";
import { draftWithOptimizedPrompt } from "../lib/promptOptimize";
import { normalizeDraftChange } from "./workspaceActions";
import { families, fieldsFor, providers, modelById } from "../models/catalog";
import { displayValue } from "../models/parameterLabels";
import { referenceWidth } from "../components/ResizableReferences";
import { applyLocale, m, resolveLocale } from "../i18n";
import { fieldLabel, modelLabel, providerLabel } from "../labels";
import type {
  Draft,
  FamilyId,
  Preferences,
  WorkspaceSession,
  WorkspaceKey,
  TaskIntent,
} from "../lib/types";

function readPreferences(): Preferences {
  try {
    const raw = JSON.parse(
      localStorage.getItem("lutriui-preferences-v2") ?? "null",
    );
    if (!raw) return DEFAULT_PREFERENCES;
    const p = {
      ...DEFAULT_PREFERENCES,
      ...raw,
      params: { ...DEFAULT_PARAMS, ...raw.params },
    } as Preferences;
    if (!providers.some((provider) => provider.id === p.provider))
      p.provider = "openrouter";
    if (!families.some((f) => f.id === p.defaultFamily))
      p.defaultFamily = "flux";
    p.sidebarWidthPercent = Math.max(
      20,
      Math.min(40, Number(p.sidebarWidthPercent) || 25),
    );
    p.referenceSidebarWidth = referenceWidth(p.referenceSidebarWidth);
    if (
      p.locale !== "system" &&
      p.locale !== "en" &&
      p.locale !== "zh" &&
      p.locale !== "ja"
    )
      p.locale = "system";
    return p;
  } catch {
    return DEFAULT_PREFERENCES;
  }
}
export function useWorkspace(
  submitting: RefObject<boolean>,
  onNotice: (s: string) => void,
  onRequestClose?: () => void,
  storageMigrating = false,
) {
  const migrating = useRef(storageMigrating);
  migrating.current = storageMigrating;
  const [prefs, setPrefsState] = useState<Preferences>(() => {
    const initial = readPreferences();
    applyLocale(resolveLocale(initial.locale));
    return initial;
  });
  const setPrefs = useCallback(
    (value: Preferences | ((current: Preferences) => Preferences)) => {
      setPrefsState((current) => {
        const next = typeof value === "function" ? value(current) : value;
        applyLocale(resolveLocale(next.locale));
        return next;
      });
    },
    [],
  );
  const [draft, setDraft] = useState<Draft>(() => newDraft(prefs));
  const current = useRef(draft);
  // A monotonically increasing epoch identifies each committed draft state;
  // the workspace version `${instanceId}:${revision}` changes on every replace.
  const instanceId = useRef(crypto.randomUUID());
  const revision = useRef(0);
  const tasks = useRef<WorkspaceSession["tasks"]>({
    [workspaceKey(draft)]: draft,
  });
  const [ready, setReady] = useState(false);
  const saveAllowed = useRef(true);
  const saveQueue = useRef(Promise.resolve());
  const [undoStack, setUndo] = useState<Draft[]>([]),
    [redoStack, setRedo] = useState<Draft[]>([]);
  const gesture = useRef(false),
    lastEdit = useRef(0);
  const histories = useRef<
    Partial<Record<WorkspaceKey, { undo: Draft[]; redo: Draft[] }>>
  >({});
  const replace = useCallback((d: Draft) => {
    revision.current += 1;
    current.current = d;
    tasks.current[workspaceKey(d)] = d;
    setDraft(d);
  }, []);
  const commit = useCallback(
    (d: Draft, discrete = false) => {
      const previous = current.current;
      if (workspaceKey(previous) !== workspaceKey(d)) {
        histories.current[workspaceKey(previous)] = {
          undo: undoStack,
          redo: redoStack,
        };
        const prior = tasks.current[workspaceKey(d)] ?? {
          ...newDraft(prefs, d.family),
          intent: taskIntent(d),
        };
        setUndo([...(histories.current[workspaceKey(d)]?.undo ?? []), prior]);
      } else if (
        !gesture.current &&
        (discrete || Date.now() - lastEdit.current > 600)
      )
        setUndo((s) => [...s.slice(-39), previous]);
      lastEdit.current = Date.now();
      setRedo([]);
      replace(withIds(d));
    },
    [replace, undoStack, redoStack, prefs],
  );
  const onChange = (next: Draft) => {
    const previous = current.current;
    const fields = fieldsFor(next);
    const discrete =
      next.provider !== previous.provider ||
      next.modelId !== previous.modelId ||
      next.intent !== previous.intent ||
      next.layoutEnabled !== previous.layoutEnabled ||
      next.repeatCount !== previous.repeatCount ||
      next.boxes.length !== previous.boxes.length ||
      next.boxes.some((box) => {
        const old = previous.boxes.find((b) => b.uid === box.uid);
        return (
          old &&
          (old.role !== box.role ||
            old.sourceId !== box.sourceId ||
            JSON.stringify(old.srcRect) !== JSON.stringify(box.srcRect))
        );
      }) ||
      !!next.mask !== !!previous.mask ||
      next.compressEnabled !== previous.compressEnabled ||
      Object.entries(fields).some(
        ([key, field]) =>
          next.params[key] !== previous.params[key] &&
          (field.kind === "enum" ||
            field.kind === "boolean" ||
            (field.nullable &&
              (next.params[key] == null) !== (previous.params[key] == null))),
      );
    const d = normalizeDraftChange(previous, next);
    commit(d, discrete);
    if (
      next.modelId !== previous.modelId ||
      next.provider !== previous.provider
    ) {
      const restored =
        previous.routeSettings?.[next.modelId + ":" + next.provider];
      const changes = Object.entries(fields).flatMap(([key, field]) =>
        key !== "count" &&
        previous.params[key] !== undefined &&
        previous.params[key] !== d.params[key]
          ? [
              `${fieldLabel(key, field.label)} ${displayValue(key, previous.params[key])} → ${displayValue(key, d.params[key])}`,
            ]
          : [],
      );
      const route = `${modelLabel(d.modelId, modelById(d.modelId)?.label ?? "")} · ${providerLabel(d.provider)}`;
      const changesText = changes.length ? "；" + changes.join("，") : "";
      onNotice(
        restored
          ? m.notice_route_restored({ route, changes: changesText })
          : m.notice_route_switched({ route, changes: changesText }),
      );
    }
  };
  const undo = () => {
    if (gesture.current) return;
    const prior = undoStack.at(-1);
    if (!prior) return;
    const previous = current.current;
    setRedo((s) => [...s, previous]);
    setUndo((s) => s.slice(0, -1));
    replace(prior);
    onNotice("");
    lastEdit.current = 0;
  };
  const redo = () => {
    if (gesture.current) return;
    const next = redoStack.at(-1);
    if (!next) return;
    const previous = current.current;
    setUndo((s) => [...s, previous]);
    setRedo((s) => s.slice(0, -1));
    replace(next);
    onNotice("");
    lastEdit.current = 0;
  };
  const draftForIntent = (intent: TaskIntent) => {
    const d = current.current;
    return (
      tasks.current[intent] ?? {
        ...newDraft(prefs, d.family),
        provider: d.provider,
        modelId: d.modelId,
        params: { ...d.params },
        intent,
      }
    );
  };
  const switchIntent = (intent: TaskIntent) => {
    if (gesture.current) return;
    const key: WorkspaceKey = intent;
    if (key === workspaceKey(current.current)) return;
    histories.current[workspaceKey(current.current)] = {
      undo: undoStack,
      redo: redoStack,
    };
    setUndo(histories.current[key]?.undo ?? []);
    setRedo(histories.current[key]?.redo ?? []);
    replace(draftForIntent(intent));
    lastEdit.current = 0;
  };
  const switchFamily = (family: FamilyId) =>
    onChange(changeFamily(current.current, family, prefs));
  const session = useCallback(
    (d = current.current): WorkspaceSession => ({
      schema: 2,
      activeIntent: taskIntent(d),
      tasks: { ...tasks.current, [taskIntent(d)]: d },
    }),
    [],
  );
  const persist = useCallback(
    async (d = current.current) => {
      if (!api.isDesktop()) return;
      await saveQueue.current.catch(() => {});
      if (saveAllowed.current) await api.draftSave(session(d));
    },
    [session],
  );
  const reset = () => {
    saveAllowed.current = true;
    commit(
      {
        ...newDraft(prefs, current.current.family),
        intent: taskIntent(current.current),
      },
      true,
    );
  };
  const closeApp = async () => {
    if (migrating.current) {
      onNotice(m.notice_library_moving());
      return;
    }
    try {
      await persist();
      await getCurrentWindow().destroy();
    } catch (e) {
      onNotice(m.notice_close_save_failed({ detail: String(e) }));
    }
  };
  useEffect(() => {
    if (!api.isDesktop()) {
      setReady(true);
      return;
    }
    let alive = true;
    api
      .draftLoad()
      .then((raw) => {
        if (!alive) return;
        const saved = readSession(raw);
        if (saved) {
          tasks.current = saved.tasks;
          replace(saved.tasks[saved.activeIntent]!);
        }
      })
      .catch((e) => {
        if (!alive) return;
        saveAllowed.current = false;
        onNotice(
          m.notice_draft_restore_failed({ detail: String(e) }),
        );
      })
      .finally(() => {
        if (alive) setReady(true);
      });
    return () => {
      alive = false;
    };
  }, [replace, onNotice]);
  useEffect(() => {
    if (!ready || !saveAllowed.current || !api.isDesktop() || storageMigrating)
      return;
    const timer = window.setTimeout(() => {
      const snapshot = session(draft);
      saveQueue.current = saveQueue.current
        .catch(() => {})
        .then(() => api.draftSave(snapshot))
        .catch((e) =>
          onNotice(m.notice_draft_save_failed({ detail: String(e) })),
        );
    }, 450);
    return () => window.clearTimeout(timer);
  }, [draft, ready, session, onNotice, storageMigrating]);
  useEffect(() => {
    if (!api.isDesktop()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    getCurrentWindow()
      .onCloseRequested(async (event) => {
        event.preventDefault();
        if (migrating.current) {
          onNotice(m.notice_library_moving());
          return;
        }
        if (submitting.current) {
          onRequestClose?.();
          return;
        }
        try {
          await persist();
          await getCurrentWindow().destroy();
        } catch (e) {
          onNotice(m.notice_close_save_blocked({ detail: String(e) }));
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else off = fn;
      })
      .catch((e) => onNotice(m.notice_close_guard_failed({ detail: String(e) })));
    return () => {
      disposed = true;
      off?.();
    };
  }, [persist, submitting, onNotice, onRequestClose]);
  useEffect(() => {
    try {
      localStorage.setItem("lutriui-preferences-v2", JSON.stringify(prefs));
    } catch (e) {
      onNotice(m.notice_prefs_save_failed({ detail: String(e) }));
    }
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => {
      document.documentElement.dataset.theme =
        prefs.theme === "system"
          ? media.matches
            ? "dark"
            : "light"
          : prefs.theme;
    };
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, [prefs, onNotice]);
  const beginGesture = () => {
    const previous = current.current;
    if (!gesture.current) setUndo((s) => [...s.slice(-39), previous]);
    gesture.current = true;
    setRedo([]);
  };
  const endGesture = () => {
    gesture.current = false;
    lastEdit.current = Date.now();
  };
  const getVersion = () => `${instanceId.current}:${revision.current}`;
  const commitRef = useRef(commit);
  commitRef.current = commit;
  const applyOptimizedPrompt = useCallback(
    (expectedVersion: string, prompt: string) => {
      const next = draftWithOptimizedPrompt(
        current.current,
        `${instanceId.current}:${revision.current}`,
        expectedVersion,
        prompt,
      );
      if (next !== current.current) commitRef.current(next, true);
    },
    [],
  );
  return {
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
    switchFamily,
    switchIntent,
    draftForIntent,
    reset,
    closeApp,
    persist,
    beginGesture,
    endGesture,
    getVersion,
    applyOptimizedPrompt,
    isGestureActive: () => gesture.current,
    canPersist: () => saveAllowed.current && !migrating.current,
  };
}
