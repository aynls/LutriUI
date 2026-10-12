import FamilySelect from "../../components/FamilySelect";
import { m } from "../../i18n";
import {
  fieldHelp,
  fieldLabel,
  generationPhase,
  modelLabel,
  providerLabel,
  valueLabel,
} from "../../labels";
import { displayValue } from "../../models/parameterLabels";
import { promptLength } from "../../models";
import {
  defaultsFor,
  fieldsFor,
  pickParams,
  modelById,
  catalog,
  providers,
  changeRoute,
  routeFor,
  singleImageDraft,
} from "../../models/catalog";
import { estimateCost } from "../../lib/params";
import { estimateComfyCredits, formatCredits } from "../../models/pricing";
import { sentSize, taskIntent } from "../../lib/workspace";
import { useEffect, useId, useRef, useState } from "react";
import { AppError, isCancelledError, isDesktop, promptOptimize } from "../../lib/api";
import {
  optimizeNeedsSettings,
  packOptimizeContext,
  takeOptimizedPrompt,
} from "../../lib/promptOptimize";
import { gptSize } from "../../models/gpt";
import type {
  Draft,
  GenerateParams,
  ProviderId,
  ProviderStatus,
  GenerationTask,
  FamilyId,
} from "../../lib/types";

export { displayValue };

export interface WorkspaceControlsProps {
  draft: Draft;
  onChange: (draft: Draft) => void;
  onFamilyChange?: (family: FamilyId) => void;
  providerStatus: ProviderStatus | null;
  busy: boolean;
  errors: string[];
  finalPreview: string;
  onSettings: () => void;
  onGenerate: (count: number) => void;
  getVersion: () => string;
  applyOptimizedPrompt: (expectedVersion: string, prompt: string) => void;
  generationTask?: GenerationTask | null;
  sentRequestId?: string;
  onShowTask?: () => void;
  onStopRemaining?: () => void;
  onCancelGeneration?: () => void;
  generationFailure?: { message: string; code?: string } | null;
}
export function RouteControls({
  draft: d,
  onChange,
  providerStatus,
  onSettings,
  onFamilyChange,
  busy,
}: Pick<
  WorkspaceControlsProps,
  | "draft"
  | "onChange"
  | "providerStatus"
  | "onSettings"
  | "onFamilyChange"
  | "busy"
>) {
  const models = catalog.models.filter((model) => model.family === d.family);
  const selected = modelById(d.modelId)!;
  const switchRoute = (provider: ProviderId, modelId = d.modelId) => {
    onChange(changeRoute(d, provider, modelId));
  };
  return (
    <section>
      <div className="section-heading">
        <h2>{m.model_and_provider()}</h2>
      </div>
      {onFamilyChange && (
        <label>
          {m.model_family()}
          <FamilySelect value={d.family} disabled={busy} onChange={onFamilyChange} />
        </label>
      )}
      {models.length > 1 && (
        <label>
          {m.model_version()}
          <select
            value={d.modelId}
            onChange={(e) => {
              const model = modelById(e.target.value)!;
              const provider = model.routes[d.provider]
                ? d.provider
                : (Object.keys(model.routes)[0] as ProviderId);
              switchRoute(provider, model.id);
            }}
          >
            {models.map((model) => (
              <option value={model.id} key={model.id}>
                {modelLabel(model.id, model.label)}
              </option>
            ))}
          </select>
        </label>
      )}
      <label>
        {m.provider()}
        <select
          value={d.provider}
          onChange={(e) => switchRoute(e.target.value as ProviderId)}
        >
          {providers
            .filter((p) => selected.routes[p.id])
            .map((p) => (
              <option key={p.id} value={p.id}>
                {providerLabel(p.id)}
              </option>
            ))}
        </select>
      </label>
      <div className="credential-hint">
        <span>
          {providerStatus?.[d.provider]
            ? m.state_configured()
            : m.state_key_needed()}
        </span>
        <button onClick={onSettings}>{m.action_settings()}</button>
      </div>
      {!!d.mask && !routeFor(d)?.mask && (
        <div className="route-warning">
          <p className="error-text">{m.mask_kept()}</p>
          <button onClick={() => onChange({ ...d, mask: null, maskRects: [] })}>
            {m.remove_mask()}
          </button>
        </div>
      )}
      {d.family !== "flux" && d.boxes.length > 0 && (
        <div className="route-warning">
          <p className={d.layoutEnabled === false ? "help" : "error-text"}>
            {d.layoutEnabled === false
              ? m.regions_kept({ count: d.boxes.length })
              : m.regions_paused_help()}
          </p>
          {d.layoutEnabled !== false && (
            <button onClick={() => onChange({ ...d, layoutEnabled: false })}>
              {m.regions_pause()}
            </button>
          )}
        </div>
      )}
    </section>
  );
}
export function ParameterFields({
  draft: d,
  onChange,
  keys,
}: {
  draft: Draft;
  onChange: (d: Draft) => void;
  keys: string[];
}) {
  const helpId = useId();
  const fields = fieldsFor(d);
  const values = { ...defaultsFor(d.modelId, d.provider), ...d.params };
  const change = (key: string, value: GenerateParams[string]) =>
    onChange({ ...d, params: { ...d.params, [key]: value } });
  return (
    <div className="parameter-fields">
      {keys
        .filter(
          (key) =>
            key !== "count" &&
            fields[key] &&
            !(key === "outputCompression" && values.outputFormat === "png") &&
            !(
              (key === "promptExtendMode" || key === "enableThinking") &&
              values.promptExtend === false
            ),
        )
        .map((key) => {
          const field = fields[key];
          const value = values[key];
          const label = fieldLabel(key, field.label);
          const help = fieldHelp(key, d.provider) ?? field.help;
          const control = (() => {
            if (field.kind === "enum" && field.values?.length === 1)
              return (
                <div className="fixed-field" key={key}>
                  <span>{label}</span>
                  <span>
                    {displayValue(key, value ?? field.values[0])}
                  </span>
                  {value != null && !field.values.includes(String(value)) && (
                    <button onClick={() => change(key, field.values![0])}>
                      {m.restore_default()}
                    </button>
                  )}
                </div>
              );
            if (key === "safetyTolerance")
              return (
                <label key={key}>
                  {label}
                  <select
                    aria-label={label}
                    value={value == null ? "default" : String(value)}
                    onChange={(e) =>
                      change(
                        key,
                        e.target.value === "default"
                          ? null
                          : Number(e.target.value),
                      )
                    }
                  >
                    <option value="default">{m.state_provider_default()}</option>
                    {Array.from({ length: (field.max ?? 4) + 1 }, (_, n) => (
                      <option key={n} value={n}>
                        {n}
                        {n === 0
                          ? m.strictest()
                          : n === field.max
                            ? m.loosest()
                            : ""}
                      </option>
                    ))}
                  </select>
                </label>
              );
            if (key === "seed")
              return (
                <div key={key} className="seed-field">
                  <label>
                    {m.seed()}
                    <select
                      aria-label={m.seed_mode()}
                      value={value == null ? "random" : "fixed"}
                      onChange={(e) =>
                        change(key, e.target.value === "random" ? null : 0)
                      }
                    >
                      <option value="random">{m.state_random()}</option>
                      <option value="fixed">{m.seed_fixed()}</option>
                    </select>
                  </label>
                  {value != null && (
                    <div className="row">
                      <label className="grow">
                        {m.seed_value()}
                        <input
                          aria-label={m.seed_value()}
                          type="number"
                          min={field.min}
                          max={field.max}
                          value={Number(value)}
                          onChange={(e) =>
                            change(
                              key,
                              e.target.value === ""
                                ? null
                                : Number(e.target.value),
                            )
                          }
                        />
                      </label>
                      <button
                        title={m.seed_roll_title()}
                        onClick={() =>
                          change(
                            key,
                            crypto.getRandomValues(new Uint32Array(1))[0] %
                              2147483648,
                          )
                        }
                      >
                        {m.seed_roll()}
                      </button>
                    </div>
                  )}
                </div>
              );
            if (field.kind === "size")
              return (
                <SizeField
                  key={key}
                  value={String(value ?? "")}
                  allowAuto={!!field.allowAuto}
                  onChange={(v) => change(key, v)}
                />
              );
            if (field.kind === "boolean")
              return (
                <label className="check" key={key}>
                  <input
                    type="checkbox"
                    checked={value === true}
                    onChange={(e) => change(key, e.target.checked)}
                  />
                  {label}
                </label>
              );
            return (
              <label key={key}>
                {label}
                {field.kind === "enum" ? (
                  <select
                    value={String(value ?? "")}
                    onChange={(e) => change(key, e.target.value)}
                  >
                    {value != null &&
                      !field.values?.includes(String(value)) && (
                        <option value={String(value)}>
                          {m.value_unused({ value: String(value) })}
                        </option>
                      )}
                    {field.values?.map((v) => (
                      <option
                        key={v}
                        value={v}
                        disabled={
                          (key === "promptExtendMode" &&
                            v === "agent" &&
                            d.refs.length > 0) ||
                          (key === "outputFormat" &&
                            v === "jpeg" &&
                            values.background === "transparent")
                        }
                      >
                        {key === "moderation" && v === "low"
                          ? m.value_loose()
                          : valueLabel(v)}
                      </option>
                    ))}
                  </select>
                ) : field.kind === "integer" ? (
                  <input
                    type="number"
                    value={value == null ? "" : Number(value)}
                    min={field.min}
                    max={field.max}
                    step={1}
                    placeholder={
                      field.nullable ? m.placeholder_default() : undefined
                    }
                    onChange={(e) =>
                      change(
                        key,
                        e.target.value === "" ? null : Number(e.target.value),
                      )
                    }
                  />
                ) : (
                  <textarea
                    rows={3}
                    value={String(value ?? "")}
                    maxLength={field.maxLength}
                    onChange={(e) => change(key, e.target.value)}
                  />
                )}
              </label>
            );
          })();
          return (
            <div
              key={key}
              className={
                "parameter-field" +
                (help ||
                ["boolean", "text", "size"].includes(field.kind) ||
                key === "seed" ||
                field.values?.length === 1
                  ? " parameter-field-wide"
                  : "")
              }
              role="group"
              aria-label={label}
              aria-describedby={help ? `${helpId}-${key}` : undefined}
            >
              {control}
              {help && (
                <p className="help" id={`${helpId}-${key}`}>
                  {help}
                </p>
              )}
            </div>
          );
        })}
    </div>
  );
}
export function SizeField({
  value,
  allowAuto,
  onChange,
}: {
  value: string;
  allowAuto: boolean;
  onChange: (value: string) => void;
}) {
  const size = gptSize(value);
  const parts = value.split(/[xX×*]/);
  return (
    <div className="size-field">
      {allowAuto && (
        <label>
          {m.output_size()}
          <select
            aria-label={m.size_mode()}
            value={value === "auto" ? "auto" : "custom"}
            onChange={(e) =>
              onChange(e.target.value === "auto" ? "auto" : "1024x1024")
            }
          >
            <option value="auto">{m.size_auto()}</option>
            <option value="custom">{m.size_custom()}</option>
          </select>
        </label>
      )}
      {value !== "auto" && (
        <>
          <div className="field-grid">
            <label>
              {m.size_width()}
              <input
                aria-label={m.size_width_label()}
                type="number"
                min={16}
                max={3840}
                step={16}
                value={size?.w ?? parts[0] ?? ""}
                onChange={(e) =>
                  onChange(`${e.target.value}x${size?.h ?? parts[1] ?? 1024}`)
                }
              />
            </label>
            <label>
              {m.size_height()}
              <input
                aria-label={m.size_height_label()}
                type="number"
                min={16}
                max={3840}
                step={16}
                value={size?.h ?? parts[1] ?? ""}
                onChange={(e) =>
                  onChange(`${size?.w ?? parts[0] ?? 1024}x${e.target.value}`)
                }
              />
            </label>
          </div>
          <div className="preset-row" aria-label={m.size_presets()}>
            {[
              [m.size_square(), "1024x1024"],
              [m.size_landscape(), "1536x1024"],
              [m.size_portrait(), "1024x1536"],
              ["2K", "2048x2048"],
            ].map(([label, v]) => (
              <button
                key={v}
                className={value === v ? "active" : ""}
                onClick={() => onChange(v)}
              >
                {label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
export function FrameSizeFields(
  p: Pick<WorkspaceControlsProps, "draft" | "onChange">,
) {
  const fields = fieldsFor(p.draft),
    auto = p.draft.params.width === null && p.draft.params.height === null;
  const customEdge =
    (routeFor(p.draft)?.minPixels ?? 0) > 1048576 ? 2048 : 1024;
  return (
    <>
      {fields.width?.nullable && (
        <label>
          {m.size_mode()}
          <select
            aria-label={m.size_mode()}
            value={auto ? "auto" : "custom"}
            onChange={(e) =>
              p.onChange({
                ...p.draft,
                params: {
                  ...p.draft.params,
                  width: e.target.value === "auto" ? null : customEdge,
                  height: e.target.value === "auto" ? null : customEdge,
                },
              })
            }
          >
            <option value="auto">{m.size_recommended()}</option>
            <option value="custom">{m.size_custom()}</option>
          </select>
        </label>
      )}
      <ParameterFields
        {...p}
        keys={[
          ...(p.draft.family === "seedream" && fields.width && !auto
            ? []
            : ["resolution"]),
          "aspectRatio",
          ...(auto ? [] : ["width", "height"]),
        ]}
      />
    </>
  );
}
export function PromptEditor({
  draft: d,
  onChange,
  onSettings,
  getVersion,
  applyOptimizedPrompt,
}: {
  draft: Draft;
  onChange: (d: Draft) => void;
  onSettings?: () => void;
  getVersion: () => string;
  applyOptimizedPrompt: (expectedVersion: string, prompt: string) => void;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const draftRef = useRef(d);
  draftRef.current = d;
  const feedbackId = useId();
  const { length, min, max, error } = promptLength(d);
  const [optimizeBusy, setOptimizeBusy] = useState(false);
  const [optimizeError, setOptimizeError] = useState("");
  const [optimizeOpenSettings, setOptimizeOpenSettings] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  useEffect(
    () => () => {
      abortRef.current?.abort();
    },
    [],
  );
  const cancelOptimize = () => {
    const abort = abortRef.current;
    abort?.abort();
    if (abortRef.current === abort) abortRef.current = null;
    setOptimizeBusy(false);
  };
  const optimize = async () => {
    if (optimizeBusy) return;
    const snapshot = draftRef.current;
    const version = getVersion();
    const abort = new AbortController();
    abortRef.current = abort;
    setOptimizeBusy(true);
    setOptimizeError("");
    setOptimizeOpenSettings(false);
    try {
      if (!isDesktop()) throw new Error(m.prompt_optimize_desktop());
      const text = (
        await promptOptimize(
          snapshot.family,
          packOptimizeContext(snapshot),
          crypto.randomUUID(),
          abort.signal,
        )
      ).trim();
      if (abort.signal.aborted) return;
      if (!text) throw new Error(m.prompt_optimize_empty());
      if (
        !takeOptimizedPrompt(
          abort.signal.aborted,
          draftRef.current,
          getVersion(),
          version,
          text,
        )
      )
        return;
      applyOptimizedPrompt(version, text);
    } catch (e) {
      if (abort.signal.aborted || isCancelledError(e)) return;
      setOptimizeError(e instanceof Error ? e.message : String(e));
      setOptimizeOpenSettings(
        optimizeNeedsSettings(e instanceof AppError ? e.code : ""),
      );
    } finally {
      if (abortRef.current === abort) {
        abortRef.current = null;
        setOptimizeBusy(false);
      }
    }
  };
  const insert = (tag: string) => {
    const start = ref.current?.selectionStart ?? d.prompt.length,
      end = ref.current?.selectionEnd ?? start;
    const value =
      (start && !/\s$/.test(d.prompt.slice(0, start)) ? " " : "") + tag + " ";
    onChange({
      ...d,
      prompt: d.prompt.slice(0, start) + value + d.prompt.slice(end),
    });
    requestAnimationFrame(() => {
      ref.current?.focus();
      ref.current?.setSelectionRange(
        start + value.length,
        start + value.length,
      );
    });
  };
  return (
    <section className="prompt-editor">
      <div className="section-heading">
        <h2>
          {d.family === "gpt" && d.refs.length
            ? m.handoff_instruction()
            : m.prompt()}
        </h2>
        <div className="in-progress-action">
          <button
            type="button"
            className={"prompt-optimize" + (optimizeBusy ? " is-busy" : "")}
            disabled={optimizeBusy}
            aria-busy={optimizeBusy || undefined}
            title={m.prompt_optimize_hint()}
            onClick={() => void optimize()}
          >
            {optimizeBusy && <span className="activity-dot" aria-hidden="true" />}
            {optimizeBusy ? m.prompt_optimize_busy() : m.prompt_optimize()}
          </button>
          {optimizeBusy && (
            <button
              type="button"
              className="in-progress-cancel"
              onClick={cancelOptimize}
            >
              {m.action_cancel()}
            </button>
          )}
        </div>
      </div>
      <textarea
        ref={ref}
        aria-label={m.prompt()}
        rows={d.family === "qwen" ? 4 : 5}
        aria-describedby={feedbackId}
        aria-invalid={!!error}
        disabled={optimizeBusy}
        placeholder={
          d.family === "qwen"
            ? m.prompt_placeholder_text()
            : m.prompt_placeholder()
        }
        value={d.prompt}
        onChange={(e) => onChange({ ...d, prompt: e.target.value })}
      />
      <div
        id={feedbackId}
        className={error ? "prompt-limit error-text" : "prompt-limit muted"}
      >
        <span>
          {error ??
            (min > 1 ? m.prompt_chars_min({ min }) : m.prompt_chars())}
        </span>
        <span>
          {length.toLocaleString()} / {max.toLocaleString()}
        </span>
      </div>
      {optimizeError && (
        <div className="prompt-optimize-error" role="alert">
          <p className="error-text">{optimizeError}</p>
          {optimizeOpenSettings && onSettings && (
            <button type="button" onClick={onSettings}>
              {m.prompt_optimize_open_settings()}
            </button>
          )}
        </div>
      )}
      {(d.refs.length > 0 || d.boxes.length > 0) && (
        <div className="tags">
          {d.refs.map((r, i) => (
            <button
              key={r.uid}
              title={r.name}
              disabled={optimizeBusy}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => insert(`<ref_image_${i}>`)}
            >
              {m.image_n({ index: i + 1 })}
            </button>
          ))}
          {d.boxes.map((b) => (
            <button
              key={b.uid}
              disabled={optimizeBusy}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => insert(`<${b.id}>`)}
            >
              {b.id}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}
export function InputOptions({
  draft: d,
  onChange,
  finalPreview,
}: Pick<WorkspaceControlsProps, "draft" | "onChange" | "finalPreview">) {
  return (
    <details>
      <summary>{m.request_preview()}</summary>
      <label className="check">
        <input
          type="checkbox"
          checked={d.compressEnabled}
          onChange={(e) =>
            onChange({ ...d, compressEnabled: e.target.checked })
          }
        />
        {m.shrink_refs()}
      </label>
      <label>
        {m.edge_limit()}
        <input
          type="number"
          min={256}
          max={8192}
          disabled={!d.compressEnabled}
          value={d.maxInputEdge}
          onChange={(e) =>
            onChange({ ...d, maxInputEdge: Number(e.target.value) })
          }
        />
      </label>
      {d.refs.map((r, i) => {
        const s = sentSize(r, d.compressEnabled, d.maxInputEdge);
        return (
          <p className="help" key={r.uid}>
            {m.send_size({
              index: i + 1,
              before: `${r.width}×${r.height}`,
              after: `${s.w}×${s.h}`,
            })}
          </p>
        );
      })}
      {d.mask && <p className="help">{m.mask_scales()}</p>}
      <h3>{m.sent_params()}</h3>
      <dl className="request-parameters">
        {Object.entries(
          pickParams(d.modelId, d.provider, {
            ...defaultsFor(d.modelId, d.provider),
            ...d.params,
          }),
        )
          .filter(
            ([key]) =>
              !(
                key === "outputCompression" && d.params.outputFormat === "png"
              ) &&
              !(
                (key === "promptExtendMode" || key === "enableThinking") &&
                d.params.promptExtend === false
              ),
          )
          .map(([key, value]) => (
            <div key={key}>
              <dt>{fieldLabel(key, catalog.fields[key]?.label ?? key)}</dt>
              <dd>{displayValue(key, value)}</dd>
            </div>
          ))}
      </dl>
      <h3>{m.compiled_prompt()}</h3>
      <pre>{finalPreview || m.compiled_empty()}</pre>
    </details>
  );
}
export function GenerateFooter(p: WorkspaceControlsProps) {
  const d = p.draft;
  const repeatCount = d.repeatCount ?? 1;
  const [sent, setSent] = useState(false);
  const [labelVisible, setLabelVisible] = useState(true);
  const lastFeedback = useRef(p.sentRequestId);
  const taskKey = `${d.family}:${taskIntent(d)}`;
  useEffect(() => {
    setSent(false);
    setLabelVisible(true);
    lastFeedback.current = p.sentRequestId;
  }, [taskKey]);
  useEffect(() => {
    if (!p.sentRequestId || p.sentRequestId === lastFeedback.current) return;
    lastFeedback.current = p.sentRequestId;
    setLabelVisible(false);
    const show = window.setTimeout(() => {
      setSent(true);
      setLabelVisible(true);
    }, 90);
    const fade = window.setTimeout(() => setLabelVisible(false), 1090);
    const restore = window.setTimeout(() => {
      setSent(false);
      setLabelVisible(true);
    }, 1180);
    return () => {
      window.clearTimeout(show);
      window.clearTimeout(fade);
      window.clearTimeout(restore);
    };
  }, [p.sentRequestId, taskKey]);
  const outputCount = repeatCount;
  const [elapsed, setElapsed] = useState(0);
  const startedAt = p.generationTask?.startedAt;
  useEffect(() => {
    if (!startedAt) return;
    const update = () =>
      setElapsed(Math.floor((Date.now() - startedAt) / 1000));
    update();
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [startedAt]);
  const job = p.generationTask;
  const jobForThis = !!job && (job.intent ?? "create") === taskIntent(d);
  const generating = jobForThis;
  const perRequestCredits = estimateComfyCredits(singleImageDraft(d));
  const credits = perRequestCredits
    ? {
        min: perRequestCredits.min * repeatCount,
        max: perRequestCredits.max * repeatCount,
      }
    : null;
  const creditLabel = credits
    ? (formatCredits(credits.min) === formatCredits(credits.max)
        ? formatCredits(credits.min)
        : formatCredits(credits.min) + "–" + formatCredits(credits.max)) +
      " Credits"
    : null;
  const perRequestCost =
    d.family === "flux" && d.provider === "openrouter"
      ? estimateCost(d.params.resolution)
      : null;
  const cost = perRequestCost == null ? null : perRequestCost * repeatCount;
  const reason = p.busy
    ? ""
    : (p.errors.find((error) => error !== m.error_prompt_required()) ??
      (!p.finalPreview
        ? ""
        : !p.providerStatus?.[d.provider]
          ? m.need_api_key()
          : ""));
  return (
    <div className="generate-footer">
      {job && (
        <div className="generation-status" role="status">
          <div>
            <span className="activity-dot" aria-hidden="true" />
            {generationPhase(job.phase)}
            <span className="muted" aria-hidden="true">
              {Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}
            </span>
          </div>
          <span className="muted">
            {modelLabel(job.modelId, modelById(job.modelId)?.label ?? job.modelId)} ·{" "}
            {providerLabel(job.provider)}
            {job.completed != null && job.total > 1
              ? m.progress_count({
                  done: job.completed,
                  total: job.total,
                })
              : ""}
          </span>
          {job.stopRequested ? (
            <span className="help">{m.tasks_stopped()}</span>
          ) : (
            !jobForThis &&
            p.onCancelGeneration && (
              <button type="button" onClick={p.onCancelGeneration}>
                {m.action_cancel()}
              </button>
            )
          )}
          {(job.intent ?? "create") !== taskIntent(d) && (
            <button onClick={p.onShowTask}>{m.back_to_task()}</button>
          )}
          {job.taskId && (
            <details>
              <summary>{m.task_details()}</summary>
              <code>{job.taskId}</code>
            </details>
          )}
        </div>
      )}
      <div className="generate-submit-row">
        <button
          className={"primary wide" + (generating ? " is-busy" : "")}
          disabled={
            generating ||
            p.busy ||
            !p.providerStatus?.[d.provider] ||
            !!p.errors.length ||
            !p.finalPreview
          }
          aria-busy={generating || undefined}
          onClick={() => p.onGenerate(repeatCount)}
          aria-describedby={reason ? "generation-reason" : undefined}
        >
          {generating ? (
            <span className="submit-label">
              <span className="activity-dot" aria-hidden="true" />
              {(taskIntent(d) === "edit"
                ? m.submit_edit_busy
                : m.submit_generate_busy)()}
            </span>
          ) : (
            <span
              className="submit-label"
              style={{ opacity: labelVisible ? 1 : 0 }}
            >
              {sent
                ? m.request_sent()
                : (taskIntent(d) === "edit" ? m.submit_edit : m.submit_generate)({
                    detail: creditLabel
                      ? m.about_credits({ amount: creditLabel })
                      : cost != null
                        ? m.about_cost({ amount: cost.toFixed(3) })
                        : m.count_images({ count: outputCount }),
                  })}
            </span>
          )}
        </button>
        {generating && p.onCancelGeneration && (
          <button
            type="button"
            className="in-progress-cancel"
            onClick={p.onCancelGeneration}
          >
            {m.action_cancel()}
          </button>
        )}
        <input
          className="generate-repeat-count"
          type="number"
          aria-label={m.output_count()}
          title={m.output_count()}
          min={1}
          max={20}
          step={1}
          value={repeatCount}
          disabled={p.busy || generating}
          onChange={(e) => {
            const value = Number(e.target.value);
            p.onChange({
              ...d,
              repeatCount: Math.max(1, Math.min(20, Math.trunc(value) || 1)),
            });
          }}
        />
      </div>
      {reason && (
        <div id="generation-reason" className="generation-reason">
          <span>{reason}</span>
          {!p.providerStatus?.[d.provider] &&
            p.finalPreview &&
            !p.errors.length && (
              <button onClick={p.onSettings}>{m.action_configure()}</button>
            )}
        </div>
      )}
      {p.generationFailure && !generating && (
        <div className="generation-failure" role="alert">
          <p className="error-text">{p.generationFailure.message}</p>
          {(p.generationFailure.code === "backend_missing_api_key" ||
            p.generationFailure.code === "backend_provider_unauthorized") && (
            <button type="button" onClick={p.onSettings}>
              {m.action_configure()}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
export function Validation({ errors }: { errors: string[] }) {
  return (
    !!errors.length && (
      <div className="validation" role="alert">
        {errors.map((e, i) => (
          <p key={i}>{e}</p>
        ))}
      </div>
    )
  );
}
