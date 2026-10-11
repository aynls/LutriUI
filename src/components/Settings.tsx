import { useEffect, useState } from "react";
import {
  credentialSave,
  credentialRemove,
  credentialCheck,
  credentialConfigure,
  historyStorage,
  libraryStats,
  llmStatus,
  llmConfigure,
  llmCredentialSave,
  llmCredentialRemove,
  llmCheck,
  type LlmStatus,
} from "../lib/api";
import { DEFAULT_PREFERENCES, newDraft } from "../lib/workspace";
import type {
  Draft,
  FamilyId,
  Preferences,
  ProviderId,
  ProviderStatus,
  CredentialSettings,
  LibraryStats,
  LibraryMaintainOp,
  MaintenanceReport,
} from "../lib/types";
import Modal from "./Modal";
import McpSettings from "./McpSettings";
import type { LibraryMigration } from "../lib/api";
import { formatBytes, formatDateTime } from "../i18n";
import { open } from "@tauri-apps/plugin-dialog";
import {
  providers,
  families,
  catalog,
  modelById,
  fieldsFor,
  changeRoute,
  validateFields,
} from "../models/catalog";
import { ParameterFields, FrameSizeFields } from "../workspaces/shared/Controls";
import { m } from "../i18n";
import { modelLabel, providerLabel } from "../labels";
import { DEFAULT_LLM_MODEL } from "../lib/promptOptimize";

export default function Settings({
  prefs,
  onChange,
  status,
  refresh,
  onClose,
  onHistory,
  initialProvider,
  storageBusy,
  storageMigrating,
  onMigrate,
  onMaintain,
}: {
  prefs: Preferences;
  onChange: (p: Preferences) => void;
  status: ProviderStatus | null;
  refresh: () => Promise<void>;
  onClose: () => void;
  onHistory: () => void;
  initialProvider?: ProviderId;
  storageBusy: boolean;
  storageMigrating: boolean;
  onMigrate: (path: string) => Promise<LibraryMigration>;
  onMaintain: (operation: LibraryMaintainOp) => Promise<MaintenanceReport>;
}) {
  const [page, setPage] = useState("connection");
  const [provider, setProvider] = useState<ProviderId>(
    initialProvider ?? prefs.provider,
  );
  const [family, setFamily] = useState<FamilyId>(prefs.defaultFamily ?? "flux");
  const [storage, setStorage] = useState("");
  const [directoryError, setDirectoryError] = useState("");
  const [libraryError, setLibraryError] = useState("");
  const [libraryMoved, setLibraryMoved] = useState("");
  const [stats, setStats] = useState<LibraryStats | null>(null);
  const [statsError, setStatsError] = useState("");
  const [maintainResult, setMaintainResult] = useState("");
  const [confirmClean, setConfirmClean] = useState(false);
  useEffect(() => {
    if (storageMigrating) return;
    historyStorage()
      .then(setStorage)
      .catch((e) => setStorage(String(e)));
  }, [storageMigrating]);
  useEffect(() => {
    if (page !== "storage" || storageMigrating) return;
    libraryStats()
      .then((value) => {
        setStats(value);
        setStatsError("");
      })
      .catch((e) => setStatsError(String(e)));
  }, [page, storageMigrating]);
  const defaults = newDraft(prefs, family),
    fields = fieldsFor(defaults);
  const updateDefaults = (d: Draft) =>
    onChange({
      ...prefs,
      familyDefaults: {
        ...prefs.familyDefaults,
        [family]: {
          modelId: d.modelId,
          provider: d.provider,
          params: d.params,
        },
      },
    });
  const chooseDirectory = async () => {
    try {
      const directory = await open({
        directory: true,
        multiple: false,
        title: m.settings_choose_dir(),
        ...(prefs.saveDirectory ? { defaultPath: prefs.saveDirectory } : {}),
      });
      if (typeof directory === "string") {
        onChange({ ...prefs, saveDirectory: directory });
        setDirectoryError("");
      }
    } catch (e) {
      setDirectoryError(m.error_choose_directory({ detail: String(e) }));
    }
  };
  const chooseLibrary = async () => {
    try {
      const directory = await open({
        directory: true,
        multiple: false,
        title: m.settings_library_choose_dir(),
      });
      if (typeof directory !== "string") return;
      setLibraryError("");
      setLibraryMoved("");
      try {
        const moved = await onMigrate(directory);
        setStorage(moved.path);
        setLibraryMoved(moved.path);
      } catch (e) {
        setLibraryError(String(e));
      }
    } catch (e) {
      setLibraryError(m.error_choose_directory({ detail: String(e) }));
    }
  };
  const reportText = (op: LibraryMaintainOp, r: MaintenanceReport) => {
    const parts =
      op === "check"
        ? m.settings_report_check({
            checked: r.checked,
            missing: r.missing,
            restored: r.restored,
          })
        : op === "rebuild_thumbnails"
          ? m.settings_report_rebuild({ count: r.rebuilt })
          : m.settings_report_cleanup({ count: r.removed });
    return r.failed.length
      ? parts +
          " " +
          m.gallery_batch_failed({ count: r.failed.length }) +
          "：" +
          r.failed.map((f) => f.message).join("；")
      : parts;
  };
  const runMaintain = async (op: LibraryMaintainOp) => {
    setMaintainResult("");
    setLibraryError("");
    setConfirmClean(false);
    try {
      const report = await onMaintain(op);
      setMaintainResult(reportText(op, report));
    } catch (e) {
      setLibraryError(String(e));
    }
    libraryStats()
      .then(setStats)
      .catch((e) => setStatsError(String(e)));
  };
  return (
    <Modal title={m.settings_title()} onClose={onClose} large>
      <div className="settings-content">
        <nav className="settings-navigation" aria-label={m.settings_nav()}>
          {[
            ["connection", m.settings_connection()],
            ["defaults", m.settings_defaults()],
            ["appearance", m.settings_appearance()],
            ["storage", m.settings_storage()],
            ["mcp", m.settings_mcp()],
          ].map(([id, label]) => (
            <button
              key={id}
              aria-current={page === id ? "page" : undefined}
              className={page === id ? "active" : ""}
              onClick={() => setPage(id)}
            >
              {label}
            </button>
          ))}
        </nav>
        <div className="settings-panel">
          {page === "connection" && (
            <>
            <LlmSettingsCard />
            <section>
              <h3>{m.settings_connection()}</h3>
              <div className="provider-selector">
                {providers.map((p) => (
                  <button
                    key={p.id}
                    className={provider === p.id ? "active" : ""}
                    aria-pressed={provider === p.id}
                    onClick={() => setProvider(p.id)}
                  >
                    <span>{providerLabel(p.id)}</span>
                    <span className="muted">
                      {status?.[p.id]
                        ? m.state_configured()
                        : m.state_missing_key()}
                    </span>
                  </button>
                ))}
              </div>
              <KeySettingsCard
                key={provider}
                provider={provider}
                status={status}
                refresh={refresh}
              />
            </section>
            </>
          )}
          {page === "defaults" && (
            <>
              <section>
                <h3>{m.settings_defaults_title()}</h3>
                <p className="muted">{m.settings_defaults_help()}</p>
                <label>
                  {m.settings_startup_family()}
                  <select
                    value={prefs.defaultFamily ?? "flux"}
                    onChange={(e) =>
                      onChange({
                        ...prefs,
                        defaultFamily: e.target.value as FamilyId,
                      })
                    }
                  >
                    {families.map((f) => (
                      <option key={f.id} value={f.id}>
                        {f.label}
                      </option>
                    ))}
                  </select>
                </label>
                <div className="segmented">
                  {families.map((f) => (
                    <button
                      key={f.id}
                      aria-pressed={family === f.id}
                      onClick={() => setFamily(f.id)}
                    >
                      {f.label}
                    </button>
                  ))}
                </div>
                <label>
                  {m.settings_default_model()}
                  <select
                    value={defaults.modelId}
                    onChange={(e) => {
                      const model = modelById(e.target.value)!;
                      updateDefaults(
                        changeRoute(
                          defaults,
                          model.routes[defaults.provider]
                            ? defaults.provider
                            : (Object.keys(model.routes)[0] as ProviderId),
                          model.id,
                        ),
                      );
                    }}
                  >
                    {catalog.models
                      .filter((model) => model.family === family)
                      .map((model) => (
                        <option key={model.id} value={model.id}>
                          {modelLabel(model.id, model.label)}
                        </option>
                      ))}
                  </select>
                </label>
                <label>
                  {m.settings_default_provider()}
                  <select
                    value={defaults.provider}
                    onChange={(e) =>
                      updateDefaults(
                        changeRoute(defaults, e.target.value as ProviderId),
                      )
                    }
                  >
                    {providers
                      .filter((p) => modelById(defaults.modelId)?.routes[p.id])
                      .map((p) => (
                        <option key={p.id} value={p.id}>
                          {providerLabel(p.id)}
                        </option>
                      ))}
                  </select>
                </label>
                {family === "qwen" && (
                  <FrameSizeFields draft={defaults} onChange={updateDefaults} />
                )}
                <ParameterFields
                  draft={defaults}
                  onChange={updateDefaults}
                  keys={Object.keys(fields).filter(
                    (k) =>
                      family !== "qwen" ||
                      ![
                        "resolution",
                        "aspectRatio",
                        "width",
                        "height",
                        "count",
                      ].includes(k),
                  )}
                />
                {validateFields(defaults).map((e) => (
                  <p key={e} className="error-text" role="alert">
                    {e}
                  </p>
                ))}
                <button
                  onClick={() =>
                    updateDefaults({
                      ...defaults,
                      params: newDraft(
                        { ...DEFAULT_PREFERENCES, provider: defaults.provider },
                        family,
                      ).params,
                    })
                  }
                >
                  {m.settings_reset_family()}
                </button>
              </section>
              <section>
                <h3>{m.settings_ref_processing()}</h3>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={prefs.compressEnabled}
                    onChange={(e) =>
                      onChange({ ...prefs, compressEnabled: e.target.checked })
                    }
                  />
                  {m.settings_shrink()}
                </label>
                <label>
                  {m.settings_edge()}
                  <input
                    type="number"
                    min={256}
                    max={8192}
                    disabled={!prefs.compressEnabled}
                    value={prefs.maxInputEdge}
                    onChange={(e) =>
                      onChange({
                        ...prefs,
                        maxInputEdge: Number(e.target.value),
                      })
                    }
                  />
                </label>
                {prefs.compressEnabled &&
                  (!Number.isInteger(prefs.maxInputEdge) ||
                    prefs.maxInputEdge < 256 ||
                    prefs.maxInputEdge > 8192) && (
                    <p className="error-text">{m.error_edge_limit_short()}</p>
                  )}
              </section>
            </>
          )}
          {page === "appearance" && (
            <section>
              <h3>{m.settings_appearance()}</h3>
              <label>
                {m.settings_theme()}
                <select
                  value={prefs.theme}
                  onChange={(e) =>
                    onChange({
                      ...prefs,
                      theme: e.target.value as Preferences["theme"],
                    })
                  }
                >
                  <option value="system">{m.theme_system()}</option>
                  <option value="light">{m.theme_light()}</option>
                  <option value="dark">{m.theme_dark()}</option>
                </select>
              </label>
              <label>
                {m.language()}
                <select
                  value={prefs.locale ?? "system"}
                  onChange={(e) =>
                    onChange({
                      ...prefs,
                      locale: e.target.value as "system" | "en" | "zh" | "ja",
                    })
                  }
                >
                  <option value="system">{m.language_system()}</option>
                  <option value="en">{m.language_en()}</option>
                  <option value="zh">{m.language_zh()}</option>
                  <option value="ja">{m.language_ja()}</option>
                </select>
              </label>
              <span className="muted">{m.settings_immediate()}</span>
            </section>
          )}
          {page === "storage" && (
            <>
              <section>
                <h3>{m.settings_save_dir()}</h3>
                <p className="storage-path">
                  {prefs.saveDirectory || m.settings_system_default()}
                </p>
                <div className="row">
                  <button onClick={() => void chooseDirectory()}>
                    {m.settings_choose_folder()}
                  </button>
                  <button
                    disabled={!prefs.saveDirectory}
                    onClick={() => onChange({ ...prefs, saveDirectory: "" })}
                  >
                    {m.settings_use_default()}
                  </button>
                </div>
                {directoryError && (
                  <p className="error-text" role="alert">
                    {directoryError}
                  </p>
                )}
              </section>
              <section>
                <h3>{m.settings_library_location()}</h3>
                <p className="storage-path">{storage}</p>
                <p className="help">{m.settings_library_help()}</p>
                <button
                  disabled={storageBusy || storageMigrating}
                  onClick={() => void chooseLibrary()}
                >
                  {m.settings_choose_folder()}
                </button>
                {storageMigrating && (
                  <p className="help" role="status">
                    {m.settings_operation_busy()}
                  </p>
                )}
                {libraryMoved && !storageMigrating && (
                  <p className="help" role="status">
                    {m.settings_library_moved({ path: libraryMoved })}
                  </p>
                )}
                {libraryError && (
                  <p className="error-text" role="alert">
                    {libraryError}
                  </p>
                )}
              </section>
              <section>
                <h3>{m.settings_library_contents()}</h3>
                {statsError && (
                  <p className="error-text" role="alert">
                    {statsError}
                  </p>
                )}
                {stats && (
                  <dl className="storage-stats">
                    <div>
                      <dt>{m.settings_stats_images()}</dt>
                      <dd>{stats.totalCount}</dd>
                    </div>
                    <div>
                      <dt>{m.settings_stats_missing()}</dt>
                      <dd>{stats.missingCount}</dd>
                    </div>
                    <div>
                      <dt>{m.settings_stats_pending()}</dt>
                      <dd>{stats.pendingDeleteCount}</dd>
                    </div>
                    <div>
                      <dt>{m.settings_stats_originals()}</dt>
                      <dd>{formatBytes(stats.originalBytes)}</dd>
                    </div>
                    <div>
                      <dt>{m.settings_stats_database()}</dt>
                      <dd>{formatBytes(stats.databaseBytes)}</dd>
                    </div>
                    <div>
                      <dt>{m.settings_stats_checked()}</dt>
                      <dd>
                        {stats.lastCheckedAt != null
                          ? formatDateTime(stats.lastCheckedAt)
                          : m.settings_stats_never()}
                      </dd>
                    </div>
                  </dl>
                )}
                <div className="row">
                  <button
                    disabled={storageBusy || storageMigrating}
                    onClick={() => void runMaintain("check")}
                  >
                    {m.settings_check_files()}
                  </button>
                  <button
                    disabled={storageBusy || storageMigrating}
                    onClick={() => void runMaintain("rebuild_thumbnails")}
                  >
                    {m.settings_rebuild_thumbs()}
                  </button>
                  <button
                    className="danger"
                    disabled={
                      storageBusy ||
                      storageMigrating ||
                      !stats ||
                      stats.missingCount === 0
                    }
                    onClick={() => setConfirmClean(true)}
                  >
                    {m.settings_clean_missing()}
                  </button>
                </div>
                {confirmClean && stats && !storageMigrating && (
                  <div className="row" role="alertdialog" aria-label={m.settings_clean_missing()}>
                    <p className="help">
                      {m.settings_clean_missing_confirm({
                        count: stats.missingCount,
                      })}
                    </p>
                    <button onClick={() => setConfirmClean(false)}>
                      {m.action_cancel()}
                    </button>
                    <button
                      className="danger"
                      onClick={() => void runMaintain("cleanup_missing")}
                    >
                      {m.settings_clean_missing()}
                    </button>
                  </div>
                )}
                {maintainResult && (
                  <p className="help" role="status">
                    {maintainResult}
                  </p>
                )}
              </section>
              <section>
                <h3>{m.settings_local_history()}</h3>
                <p className="storage-path">{storage}</p>
                <button onClick={onHistory}>{m.settings_manage_history()}</button>
              </section>
            </>
          )}
          {page === "mcp" && <McpSettings />}
        </div>
      </div>
    </Modal>
  );
}
function LlmSettingsCard() {
  const [status, setStatus] = useState<LlmStatus | null>(null);
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const load = async () => {
    const next = await llmStatus();
    setStatus(next);
    setBaseUrl(next.baseUrl);
    setModel(next.model.trim() || DEFAULT_LLM_MODEL);
  };
  useEffect(() => {
    load().catch((e) => setError(String(e)));
  }, []);
  const effectiveModel = model.trim() || DEFAULT_LLM_MODEL;
  const changed =
    !!status &&
    (baseUrl.trim() !== status.baseUrl ||
      effectiveModel !== (status.model.trim() || DEFAULT_LLM_MODEL));
  const run = async (action: "save" | "saveKey" | "remove" | "check") => {
    setBusy(true);
    setMessage("");
    setError("");
    try {
      if (action === "save" || action === "saveKey") {
        await llmConfigure(baseUrl, effectiveModel);
      }
      if (action === "saveKey") {
        await llmCredentialSave(key);
        setKey("");
      }
      if (action === "remove") await llmCredentialRemove();
      if (action === "check") setMessage(await llmCheck());
      else
        setMessage(
          action === "remove"
            ? m.credential_removed()
            : action === "saveKey"
              ? m.credential_saved()
              : m.settings_llm_saved(),
        );
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section>
      <div className="section-heading">
        <h3>{m.settings_llm()}</h3>
        <span className="muted">
          {status?.configured ? m.state_configured() : m.state_missing_key()}
        </span>
      </div>
      <p className="help">{m.settings_llm_help()}</p>
      <label>
        {m.settings_llm_url()}
        <input
          disabled={busy}
          autoComplete="off"
          spellCheck={false}
          value={baseUrl}
          placeholder={m.settings_llm_url_placeholder()}
          onChange={(e) => {
            setBaseUrl(e.target.value);
            setMessage("");
            setError("");
          }}
        />
      </label>
      <label>
        {m.settings_llm_model()}
        <input
          disabled={busy}
          autoComplete="off"
          spellCheck={false}
          value={model}
          placeholder={m.settings_llm_model_placeholder()}
          onChange={(e) => {
            setModel(e.target.value);
            setMessage("");
            setError("");
          }}
        />
      </label>
      <label>
        {m.settings_llm_key()}
        <input
          disabled={busy}
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={key}
          placeholder={
            status?.hasKey
              ? m.credential_stored_hint()
              : m.credential_clear_hint()
          }
          onChange={(e) => setKey(e.target.value)}
        />
      </label>
      <span className="muted">{m.credential_store()}</span>
      {changed && <p className="help">{m.settings_llm_pending()}</p>}
      <div className="row">
        <button
          disabled={busy || !status || !changed}
          onClick={() => void run("save")}
        >
          {m.settings_llm_save()}
        </button>
        <button
          disabled={busy || !key.trim()}
          onClick={() => void run("saveKey")}
        >
          {m.credential_save()}
        </button>
        <button
          disabled={busy || !status?.hasKey}
          onClick={() => void run("remove")}
        >
          {m.credential_remove()}
        </button>
        <button
          disabled={
            busy || !status || !status.hasKey || !status.baseUrl.trim() || changed
          }
          onClick={() => void run("check")}
        >
          {m.credential_check()}
        </button>
      </div>
      <p className="help">{m.settings_llm_check_help()}</p>
      {error && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {message && (
        <p className="help" role="status">
          {message}
        </p>
      )}
    </section>
  );
}
function KeySettingsCard({
  provider,
  status,
  refresh,
}: {
  provider: ProviderId;
  status: ProviderStatus | null;
  refresh: () => Promise<void>;
}) {
  const definition = providers.find((p) => p.id === provider)!;
  const defaultName = definition.envName;
  const active: CredentialSettings = status?.settings?.[provider] ?? {
    source: status?.sources?.[provider] === "system" ? "manual" : "environment",
    envName: defaultName,
  };
  const [selection, setSelection] = useState<CredentialSettings>(active);
  const [key, setKey] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setSelection({ source: active.source, envName: active.envName });
  }, [active.source, active.envName]);
  const changed =
    selection.source !== active.source ||
    selection.envName.trim() !== active.envName;
  const validName =
    !!selection.envName.trim() && !/[=\0\r\n]/.test(selection.envName);
  const configured = !!status?.[provider];
  const stored =
    status?.storedKeys?.[provider] ?? status?.sources?.[provider] === "system";
  const run = async (action: "apply" | "save" | "remove" | "check") => {
    setBusy(true);
    setMessage("");
    try {
      const config = {
        ...selection,
        envName: selection.envName.trim() || defaultName,
      };
      if (action === "save") {
        await credentialSave(provider, key);
        setKey("");
        await credentialConfigure(provider, { ...config, source: "manual" });
        setMessage(m.credential_saved());
      }
      if (action === "apply") {
        await credentialConfigure(provider, config);
        setMessage(m.credential_source_applied());
      }
      if (action === "remove") {
        await credentialRemove(provider);
        setMessage(m.credential_removed());
      }
      if (action === "check") setMessage(await credentialCheck(provider));
      await refresh();
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="key-settings">
      <div className="section-heading">
        <strong>{providerLabel(definition.id)}</strong>
        <span className="muted">
          {configured ? m.state_configured() : m.state_missing_key()}
        </span>
      </div>
      <label>
        {m.credential_source()}
        <select
          disabled={busy || !status}
          value={selection.source}
          onChange={(e) => {
            setSelection((s) => ({
              ...s,
              source: e.target.value as CredentialSettings["source"],
            }));
            setMessage("");
          }}
        >
          <option value="environment">{m.credential_environment()}</option>
          <option value="manual">{m.credential_manual()}</option>
        </select>
      </label>
      {selection.source === "environment" ? (
        <>
          <label>
            {m.credential_env_name()}
            <input
              disabled={busy}
              autoComplete="off"
              spellCheck={false}
              value={selection.envName}
              placeholder={defaultName}
              onChange={(e) => {
                setSelection((s) => ({ ...s, envName: e.target.value }));
                setMessage("");
              }}
            />
          </label>
          {!validName && (
            <p className="error-text">{m.error_env_name()}</p>
          )}
        </>
      ) : (
        <>
          <label>
            {m.credential_replace()}
            <input
              disabled={busy}
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={key}
              placeholder={
                stored
                  ? m.credential_stored_hint()
                  : m.credential_clear_hint()
              }
              onChange={(e) => setKey(e.target.value)}
            />
          </label>
          <span className="muted">{m.credential_store()}</span>
        </>
      )}
      {changed && <p className="help">{m.credential_pending()}</p>}
      <div className="row">
        <button
          disabled={
            busy ||
            !status ||
            !changed ||
            (selection.source === "environment" && !validName)
          }
          onClick={() => void run("apply")}
        >
          {m.credential_apply()}
        </button>
        {selection.source === "manual" && (
          <>
            <button
              disabled={busy || !key.trim()}
              onClick={() => void run("save")}
            >
              {m.credential_save()}
            </button>
            <button
              disabled={busy || !stored}
              onClick={() => void run("remove")}
            >
              {m.credential_remove()}
            </button>
          </>
        )}
        <button
          disabled={
            busy ||
            !configured ||
            changed ||
            provider === "ark" ||
            provider === "byteplus"
          }
          title={
            provider === "ark" || provider === "byteplus"
              ? m.credential_check_help()
              : undefined
          }
          onClick={() => void run("check")}
        >
          {m.credential_check()}
        </button>
      </div>
      {message && (
        <p className="help" role="status">
          {message}
        </p>
      )}
    </div>
  );
}
