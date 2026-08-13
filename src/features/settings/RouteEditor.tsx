import {
  Eye,
  EyeOff,
  FileCode2,
  LoaderCircle,
  Plus,
  Route,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  checkRouteReachability,
  deleteRoute,
  getRouteEdit,
  normalizeIpcError,
  saveRoute,
  testBalanceQuery,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type {
  BalanceQueryMode,
  BalanceResult,
  CodexModelDto,
  ReachabilityResult,
  RouteEditDto,
  RouteId,
  RouteSaveInputDto,
  ServiceTierPolicy,
} from "../../generated";
import { AppScrollArea } from "../shared/AppScrollArea";
import { previewBaseUrl } from "./baseUrlPreview";
import customBalanceScriptScaffold from "./customBalanceScriptScaffold.txt?raw";
import { formatBalanceScript } from "./formatBalanceScript";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsDivider,
  SettingsFieldRow,
  SettingsFooter,
  SettingsIconButton,
  SettingsPageTitle,
  SettingsReadonlyRow,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextarea,
  SettingsTextInput,
  type SettingsConfirmation,
  type SettingsTone,
} from "./SettingsPrimitives";

interface CodexModelDraft {
  key: string;
  modelId: string;
  displayName: string;
  contextWindow: string;
}

type CodexModelField = "modelId" | "displayName" | "contextWindow";
type CodexModelErrors = Record<
  string,
  Partial<Record<CodexModelField, string>>
>;

function codexModelDrafts(models: CodexModelDto[]): CodexModelDraft[] {
  return models.map((model, index) => ({
    key: `saved-model-${index}`,
    modelId: model.modelId,
    displayName: model.displayName ?? "",
    contextWindow:
      model.contextWindow === null ? "" : String(model.contextWindow),
  }));
}

function containsControlCharacter(value: string) {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0);
    return (
      codePoint !== undefined &&
      (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f))
    );
  });
}

function validateCodexModels(rows: CodexModelDraft[]): CodexModelErrors {
  const errors: CodexModelErrors = {};
  const ids = new Map<string, string>();
  for (const row of rows) {
    const rowErrors: Partial<Record<CodexModelField, string>> = {};
    const modelId = row.modelId.trim();
    if (!modelId) rowErrors.modelId = "请输入模型 ID。";
    else if (containsControlCharacter(modelId)) {
      rowErrors.modelId = "不能包含控制字符。";
    } else if (ids.has(modelId)) rowErrors.modelId = "模型 ID 不能重复。";
    else ids.set(modelId, row.key);
    if (containsControlCharacter(row.displayName.trim())) {
      rowErrors.displayName = "不能包含控制字符。";
    }
    if (row.contextWindow.trim()) {
      const contextWindow = Number(row.contextWindow);
      if (!Number.isSafeInteger(contextWindow) || contextWindow <= 0) {
        rowErrors.contextWindow = "请输入正整数。";
      }
    }
    if (Object.keys(rowErrors).length > 0) errors[row.key] = rowErrors;
  }
  return errors;
}

function modelActivationMessage(activation: string) {
  switch (activation) {
    case "restart_codex":
      return "已保存，重启 Codex 后生效";
    case "connect_codex":
      return "已保存，连接 Codex 后生效";
    case "reconnect_codex":
      return "已保存，需要重新连接 Codex";
    case "fix_codex_config":
      return "已保存，请先修复 Codex 配置";
    default:
      return "已保存";
  }
}

interface RouteFormState {
  name: string;
  baseUrl: string;
  apiKey: string;
  serviceTierPolicy: ServiceTierPolicy;
  queryMode: BalanceQueryMode;
  queryEnabled: boolean;
  customSource: string;
  models: CodexModelDraft[];
}

type BalanceTestFeedback =
  | { kind: "result"; result: BalanceResult }
  | { kind: "error"; message: string };

const emptyRouteForm: RouteFormState = {
  name: "",
  baseUrl: "",
  apiKey: "",
  serviceTierPolicy: "passthrough",
  queryMode: "general_v1",
  queryEnabled: false,
  customSource: "",
  models: [],
};

function formFromEdit(edit: RouteEditDto): RouteFormState {
  return {
    name: edit.name,
    baseUrl: edit.baseUrl,
    apiKey: edit.apiKey,
    serviceTierPolicy: edit.serviceTierPolicy,
    queryMode: edit.balanceQuery?.mode ?? "general_v1",
    queryEnabled: edit.balanceQuery?.enabled ?? false,
    customSource: edit.balanceQuery?.customSource ?? "",
    models: codexModelDrafts(edit.models),
  };
}

export function RouteEditor(props: {
  routeId: RouteId | null;
  newRoute: boolean;
  activeRouteId: RouteId | null;
  riskConfirmed: boolean;
  externalBusy: boolean;
  onDirtyChange: (dirty: boolean) => void;
  onCancel: () => void;
  onSaved: (routeId: RouteId) => void;
  onDeleted: (routeId: RouteId) => void;
}) {
  const [edit, setEdit] = useState<RouteEditDto | null>(null);
  const [loadState, setLoadState] = useState<"loading" | "ready" | "error">(
    props.newRoute || props.routeId === null ? "ready" : "loading",
  );

  useEffect(() => {
    if (props.newRoute || props.routeId === null) return undefined;
    let disposed = false;
    void getRouteEdit(props.routeId)
      .then((result) => {
        if (disposed) return;
        setEdit(result);
        setLoadState("ready");
      })
      .catch(() => {
        if (!disposed) setLoadState("error");
      });
    return () => {
      disposed = true;
    };
  }, [props.newRoute, props.routeId]);

  if (!props.newRoute && props.routeId === null) {
    return (
      <section className="route-editor-empty">
        <Route aria-hidden="true" size={24} />
        <h2>选择或新建路由</h2>
      </section>
    );
  }
  if (!props.newRoute && loadState === "loading")
    return <section className="route-editor-empty">正在读取路由...</section>;
  if (!props.newRoute && (loadState === "error" || edit === null))
    return (
      <section className="route-editor-empty settings-error">
        路由读取失败。
      </section>
    );
  return (
    <RouteForm
      {...props}
      initial={
        props.newRoute ? emptyRouteForm : formFromEdit(edit as RouteEditDto)
      }
    />
  );
}

function RouteForm(props: {
  routeId: RouteId | null;
  newRoute: boolean;
  activeRouteId: RouteId | null;
  riskConfirmed: boolean;
  externalBusy: boolean;
  initial: RouteFormState;
  onDirtyChange: (dirty: boolean) => void;
  onCancel: () => void;
  onSaved: (routeId: RouteId) => void;
  onDeleted: (routeId: RouteId) => void;
}) {
  const { onDirtyChange } = props;
  const queryClient = useQueryClient();
  const [form, setForm] = useState<RouteFormState>(() => props.initial);
  const [baseline, setBaseline] = useState<RouteFormState>(() => props.initial);
  const [showKey, setShowKey] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reachability, setReachability] = useState<ReachabilityResult | null>(
    null,
  );
  const [balanceFeedback, setBalanceFeedback] =
    useState<BalanceTestFeedback | null>(null);
  const [confirmation, setSettingsConfirmation] =
    useState<SettingsConfirmation | null>(null);
  const [modelErrors, setModelErrors] = useState<CodexModelErrors>({});
  const [retryToken, setRetryToken] = useState<string | null>(null);
  const [modelSuccess, setModelSuccess] = useState<string | null>(null);
  const nextModelKey = useRef(props.initial.models.length);
  const pendingModelFocusKey = useRef<string | null>(null);
  const modelIdRefs = useRef(new Map<string, HTMLInputElement>());
  const probeGeneration = useRef(0);
  const dirty =
    retryToken !== null || JSON.stringify(form) !== JSON.stringify(baseline);
  const modelsDirty =
    retryToken !== null ||
    JSON.stringify(form.models) !== JSON.stringify(baseline.models);
  const locked = busy || props.externalBusy;

  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);
  useEffect(() => {
    const key = pendingModelFocusKey.current;
    if (!key) return;
    modelIdRefs.current.get(key)?.focus();
    pendingModelFocusKey.current = null;
  }, [form.models]);

  const baseUrlPreview = useMemo(
    () => previewBaseUrl(form.baseUrl),
    [form.baseUrl],
  );
  const inferenceUrl = form.baseUrl.trim()
    ? baseUrlPreview.valid
      ? baseUrlPreview.inferenceUrl
      : "地址无效"
    : "";
  const insecureHttp = useMemo(() => {
    try {
      const url = new URL(form.baseUrl);
      return (
        url.protocol === "http:" &&
        !["127.0.0.1", "localhost"].includes(url.hostname)
      );
    } catch {
      return false;
    }
  }, [form.baseUrl]);

  const patchForm = <K extends keyof RouteFormState>(
    key: K,
    value: RouteFormState[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    if (key === "baseUrl") {
      probeGeneration.current += 1;
      setReachability(null);
    }
    if (["baseUrl", "apiKey", "queryMode", "customSource"].includes(key)) {
      setBalanceFeedback(null);
    }
    setError(null);
  };

  const patchModel = (key: string, field: CodexModelField, value: string) => {
    setForm((current) => ({
      ...current,
      models: current.models.map((row) =>
        row.key === key ? { ...row, [field]: value } : row,
      ),
    }));
    setModelErrors((current) => {
      if (!current[key]?.[field]) return current;
      return { ...current, [key]: { ...current[key], [field]: undefined } };
    });
    setError(null);
    setModelSuccess(null);
  };

  const addModel = () => {
    const key = `new-model-${nextModelKey.current++}`;
    pendingModelFocusKey.current = key;
    setForm((current) => ({
      ...current,
      models: [
        ...current.models,
        { key, modelId: "", displayName: "", contextWindow: "" },
      ],
    }));
    setError(null);
    setModelSuccess(null);
  };

  const removeModel = (key: string) => {
    setForm((current) => ({
      ...current,
      models: current.models.filter((row) => row.key !== key),
    }));
    setModelErrors((current) => {
      const next = { ...current };
      delete next[key];
      return next;
    });
    setError(null);
    setModelSuccess(null);
  };

  const commitSave = async (acceptScriptRisk: boolean) => {
    if (locked) return;
    const validation = validateCodexModels(form.models);
    setModelErrors(validation);
    if (Object.keys(validation).length > 0) return;
    setBusy(true);
    setError(null);
    try {
      const balanceQuery =
        form.customSource ||
        form.queryEnabled ||
        form.queryMode !== "general_v1"
          ? {
              mode: form.queryMode,
              enabled: form.queryEnabled,
              customSource: form.customSource,
            }
          : null;
      const input: RouteSaveInputDto = {
        routeId: props.newRoute ? null : props.routeId,
        name: form.name,
        baseUrl: form.baseUrl,
        apiKey: form.apiKey,
        serviceTierPolicy: form.serviceTierPolicy,
        balanceQuery,
        acceptScriptRisk,
        models: form.models.map((row) => ({
          modelId: row.modelId.trim(),
          displayName: row.displayName.trim() || null,
          contextWindow: row.contextWindow.trim()
            ? Number(row.contextWindow)
            : null,
        })),
        retryToken,
      };
      const result = await saveRoute(input);
      if (result.catalog.retryRequired && result.catalog.retryToken) {
        setRetryToken(result.catalog.retryToken);
        setError("路由已保存，但模型目录尚未完整应用。请重试。");
        return;
      }
      const savedModels = codexModelDrafts(result.catalog.models).map(
        (model, index) => ({
          ...model,
          key: form.models[index]?.key ?? model.key,
        }),
      );
      const canonicalBaseUrl = previewBaseUrl(form.baseUrl);
      const savedForm = {
        ...form,
        baseUrl: canonicalBaseUrl.valid
          ? canonicalBaseUrl.canonicalPrefix
          : form.baseUrl,
        models: savedModels,
      };
      setForm(savedForm);
      setBaseline(savedForm);
      setRetryToken(null);
      setModelSuccess(modelActivationMessage(result.catalog.activation));
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
        queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
        queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
      ]);
      props.onSaved(result.routeId);
    } catch (reason) {
      const normalized = normalizeIpcError(reason);
      const match = normalized.field?.match(/^models\.(\d+)\.(.+)$/u);
      if (match) {
        const row = form.models[Number(match[1])];
        const field = match[2] as CodexModelField;
        if (
          row &&
          ["modelId", "displayName", "contextWindow"].includes(field)
        ) {
          setModelErrors((current) => ({
            ...current,
            [row.key]: { ...current[row.key], [field]: normalized.message },
          }));
        }
      }
      setError(normalized.message);
    } finally {
      setBusy(false);
    }
  };

  const save = () => {
    if (
      form.queryMode === "custom_js" &&
      form.queryEnabled &&
      !props.riskConfirmed
    ) {
      setSettingsConfirmation({
        title: "允许余额脚本使用 API Key？",
        body: "自定义脚本可以把当前路由的 API Key 发送到任意 HTTP(S) 地址。请只启用你信任的脚本。",
        confirmLabel: "确认并保存",
        onConfirm: () => {
          setSettingsConfirmation(null);
          void commitSave(true);
        },
      });
      return;
    }
    void commitSave(false);
  };

  const probe = async () => {
    const generation = ++probeGeneration.current;
    setBusy(true);
    setError(null);
    try {
      const result = await checkRouteReachability(form.baseUrl);
      if (generation === probeGeneration.current) setReachability(result);
    } catch (reason) {
      if (generation === probeGeneration.current) {
        setError(normalizeIpcError(reason).message);
      }
    } finally {
      setBusy(false);
    }
  };

  const testBalance = async () => {
    setBusy(true);
    setError(null);
    setBalanceFeedback(null);
    try {
      setBalanceFeedback({
        kind: "result",
        result: await testBalanceQuery({
          baseUrl: form.baseUrl,
          apiKey: form.apiKey,
          mode: form.queryMode,
          customSource: form.customSource,
        }),
      });
    } catch (reason) {
      setBalanceFeedback({
        kind: "error",
        message: normalizeIpcError(reason).message,
      });
    } finally {
      setBusy(false);
    }
  };

  const formatScript = async () => {
    setBusy(true);
    setError(null);
    try {
      patchForm("customSource", await formatBalanceScript(form.customSource));
    } catch {
      setError("脚本格式化失败，请检查 JavaScript 语法。");
    } finally {
      setBusy(false);
    }
  };

  const requestDelete = () => {
    if (!props.routeId || locked) return;
    const active = props.routeId === props.activeRouteId;
    setSettingsConfirmation({
      title: active
        ? `删除当前路由“${form.name}”？`
        : `删除路由“${form.name}”？`,
      body: active
        ? "删除后将进入“无中转”，新请求会失败，直到你手动切换路由。"
        : "这条路由及其 API Key 和余额脚本将被永久删除。",
      confirmLabel: "删除路由",
      destructive: true,
      onConfirm: () => {
        const routeId = props.routeId as RouteId;
        setSettingsConfirmation(null);
        setBusy(true);
        void deleteRoute(routeId)
          .then(async () => {
            await Promise.all([
              queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
              queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
              queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
            ]);
            props.onDeleted(routeId);
          })
          .catch((reason) => setError(normalizeIpcError(reason).message))
          .finally(() => setBusy(false));
      },
    });
  };

  return (
    <section
      className="route-form-pane"
      aria-label={props.newRoute ? "新建路由" : `编辑 ${form.name}`}
    >
      <AppScrollArea
        className="route-form-scroll"
        viewportClassName="route-form-scroll-viewport"
      >
        <fieldset className="route-form-fields" disabled={locked}>
          <SettingsPageTitle title={props.newRoute ? "新建路由" : form.name} />
          <SettingsFieldRow label="路由名称" htmlFor="route-name">
            <SettingsTextInput
              id="route-name"
              value={form.name}
              maxLength={30}
              onChange={(event) => patchForm("name", event.target.value)}
            />
          </SettingsFieldRow>
          <SettingsFieldRow label="Responses Base URL" htmlFor="route-base-url">
            <SettingsTextInput
              id="route-base-url"
              value={form.baseUrl}
              onChange={(event) => patchForm("baseUrl", event.target.value)}
              placeholder="https://example.com/v1"
            />
          </SettingsFieldRow>
          <SettingsReadonlyRow label="最终推理地址">
            {inferenceUrl || "-"}
          </SettingsReadonlyRow>
          {insecureHttp ? (
            <p className="inline-warning">
              该地址使用非回环 HTTP，API Key 将以明文传输。
            </p>
          ) : null}
          <SettingsFieldRow label="API Key" htmlFor="route-api-key">
            <span className="secret-input">
              <SettingsTextInput
                id="route-api-key"
                type={showKey ? "text" : "password"}
                value={form.apiKey}
                onChange={(event) => patchForm("apiKey", event.target.value)}
              />
              <SettingsIconButton
                type="button"
                label={showKey ? "隐藏 API Key" : "显示 API Key"}
                title={showKey ? "隐藏 Key" : "显示 Key"}
                onClick={() => setShowKey((value) => !value)}
              >
                {showKey ? (
                  <EyeOff aria-hidden="true" size={16} />
                ) : (
                  <Eye aria-hidden="true" size={16} />
                )}
              </SettingsIconButton>
            </span>
          </SettingsFieldRow>
          <SettingsFieldRow label="Service Tier">
            <div
              className="settings-segments"
              role="radiogroup"
              aria-label="Service Tier"
            >
              <label className="settings-segment-option">
                <input
                  id="route-service-tier-passthrough"
                  type="radio"
                  name="route-service-tier-policy"
                  value="passthrough"
                  checked={form.serviceTierPolicy === "passthrough"}
                  onChange={() => patchForm("serviceTierPolicy", "passthrough")}
                />
                <span>跟随 Codex</span>
              </label>
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-service-tier-policy"
                  value="omit"
                  checked={form.serviceTierPolicy === "omit"}
                  onChange={() => patchForm("serviceTierPolicy", "omit")}
                />
                <span>移除参数</span>
              </label>
            </div>
          </SettingsFieldRow>
          <SettingsActionGroup className="route-probe-actions">
            <SettingsButton
              type="button"
              disabled={busy || !baseUrlPreview.valid}
              onClick={() => void probe()}
            >
              检查推理地址
            </SettingsButton>
            {reachability ? (
              <SettingsStatus
                tone={reachabilityPresentation[reachability.status].tone}
              >
                {reachabilityPresentation[reachability.status].label}
                {reachability.ttfbMs !== null
                  ? ` · ${reachability.ttfbMs} ms`
                  : null}
              </SettingsStatus>
            ) : null}
          </SettingsActionGroup>
          <SettingsDivider />
          <section
            className="route-model-section"
            aria-labelledby="route-model-heading"
          >
            <div className="settings-section-heading">
              <h3 id="route-model-heading" className="settings-section-title">
                自定义模型
              </h3>
              <SettingsStatus
                tone={
                  retryToken ? "danger" : modelSuccess ? "success" : "neutral"
                }
              >
                {modelSuccess ??
                  (retryToken
                    ? `${form.models.length} 个 · 需要重试`
                    : modelsDirty
                      ? `${form.models.length} 个 · 未保存`
                      : `${form.models.length} 个`)}
              </SettingsStatus>
            </div>
            <p className="codex-model-notice">
              保存非空列表后，将替换 Codex 内置模型列表。
            </p>
            {form.models.length === 0 ? (
              <div className="codex-model-empty">尚未添加自定义模型</div>
            ) : (
              <div className="codex-model-grid" aria-label="自定义模型列表">
                <div className="codex-model-grid-header" aria-hidden="true">
                  <span>模型 ID</span>
                  <span>显示名称</span>
                  <span>上下文窗口（Token）</span>
                  <span />
                </div>
                {form.models.map((row, index) => {
                  const rowErrors = modelErrors[row.key] ?? {};
                  return (
                    <div className="codex-model-row" key={row.key}>
                      <div className="codex-model-field">
                        <SettingsTextInput
                          ref={(node: HTMLInputElement | null) => {
                            if (node) modelIdRefs.current.set(row.key, node);
                            else modelIdRefs.current.delete(row.key);
                          }}
                          aria-label={`模型 ID ${index + 1}`}
                          aria-invalid={Boolean(rowErrors.modelId)}
                          value={row.modelId}
                          disabled={busy}
                          onChange={(event) =>
                            patchModel(
                              row.key,
                              "modelId",
                              event.currentTarget.value,
                            )
                          }
                        />
                        {rowErrors.modelId ? (
                          <span role="alert">{rowErrors.modelId}</span>
                        ) : null}
                      </div>
                      <div className="codex-model-field">
                        <SettingsTextInput
                          aria-label={`显示名称 ${index + 1}`}
                          aria-invalid={Boolean(rowErrors.displayName)}
                          placeholder="使用模型 ID"
                          value={row.displayName}
                          disabled={busy}
                          onChange={(event) =>
                            patchModel(
                              row.key,
                              "displayName",
                              event.currentTarget.value,
                            )
                          }
                        />
                        {rowErrors.displayName ? (
                          <span role="alert">{rowErrors.displayName}</span>
                        ) : null}
                      </div>
                      <div className="codex-model-field">
                        <SettingsTextInput
                          aria-label={`上下文窗口（Token） ${index + 1}`}
                          aria-invalid={Boolean(rowErrors.contextWindow)}
                          type="number"
                          min={1}
                          step={1}
                          placeholder="128000"
                          value={row.contextWindow}
                          disabled={busy}
                          onChange={(event) =>
                            patchModel(
                              row.key,
                              "contextWindow",
                              event.currentTarget.value,
                            )
                          }
                        />
                        {rowErrors.contextWindow ? (
                          <span role="alert">{rowErrors.contextWindow}</span>
                        ) : null}
                      </div>
                      <SettingsIconButton
                        type="button"
                        label={`删除模型 ${index + 1}`}
                        title={`删除模型 ${index + 1}`}
                        disabled={busy}
                        onClick={() => removeModel(row.key)}
                      >
                        <Trash2 aria-hidden="true" size={15} />
                      </SettingsIconButton>
                    </div>
                  );
                })}
              </div>
            )}
            <div className="codex-model-actions route-model-actions">
              <SettingsButton type="button" disabled={busy} onClick={addModel}>
                <Plus aria-hidden="true" size={15} />
                添加模型
              </SettingsButton>
            </div>
          </section>

          <SettingsDivider />
          <h3 className="settings-section-title">余额查询</h3>
          <SettingsFieldRow label="查询方式">
            <div
              className="settings-segments"
              role="radiogroup"
              aria-label="余额查询方式"
            >
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-balance-query-mode"
                  value="general_v1"
                  checked={form.queryMode === "general_v1"}
                  onChange={() => patchForm("queryMode", "general_v1")}
                />
                <span>通用查询</span>
              </label>
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-balance-query-mode"
                  value="custom_js"
                  checked={form.queryMode === "custom_js"}
                  onChange={() => patchForm("queryMode", "custom_js")}
                />
                <span>自定义脚本</span>
              </label>
            </div>
          </SettingsFieldRow>
          <SettingsSwitch
            label="启用余额查询"
            checked={form.queryEnabled}
            onChange={(event) =>
              patchForm("queryEnabled", event.target.checked)
            }
          />
          {form.queryMode === "custom_js" ? (
            <SettingsTextarea
              aria-label="JavaScript 表达式"
              className="route-script-editor"
              spellCheck={false}
              value={form.customSource}
              onChange={(event) =>
                patchForm("customSource", event.target.value)
              }
              placeholder="({ request, extractor })"
            />
          ) : null}
          <SettingsActionGroup className="route-script-actions">
            {form.queryMode === "custom_js" ? (
              form.customSource ? (
                <SettingsButton
                  type="button"
                  disabled={busy}
                  onClick={() => void formatScript()}
                >
                  <Sparkles aria-hidden="true" size={15} />
                  格式化
                </SettingsButton>
              ) : (
                <SettingsButton
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    patchForm("customSource", customBalanceScriptScaffold)
                  }
                >
                  <FileCode2 aria-hidden="true" size={15} />
                  插入骨架
                </SettingsButton>
              )
            ) : null}
            <div className="route-balance-actions">
              {balanceFeedback ? (
                <SettingsStatus
                  className="balance-test-result"
                  role={balanceFeedback.kind === "error" ? "alert" : "status"}
                  tone={
                    balanceFeedback.kind === "result" &&
                    balanceFeedback.result.isValid
                      ? "success"
                      : "danger"
                  }
                >
                  {balanceFeedback.kind === "error"
                    ? balanceFeedback.message
                    : balanceFeedback.result.isValid
                      ? `余额 ${balanceFeedback.result.remaining === null ? "-" : balanceFeedback.result.remaining.toFixed(2)} ${balanceFeedback.result.unit ?? ""}`
                      : (balanceFeedback.result.invalidMessage ??
                        "查询返回无效结果")}
                </SettingsStatus>
              ) : null}
              <SettingsButton
                variant="primary"
                type="button"
                disabled={
                  busy ||
                  !form.apiKey ||
                  !form.baseUrl ||
                  (form.queryMode === "custom_js" && !form.customSource)
                }
                onClick={() => void testBalance()}
              >
                测试余额查询
              </SettingsButton>
            </div>
          </SettingsActionGroup>
          {error ? (
            <p className="settings-error" role="alert">
              {error}
            </p>
          ) : null}
        </fieldset>
      </AppScrollArea>
      <SettingsFooter
        leading={
          props.newRoute ? null : (
            <SettingsButton
              variant="danger-link"
              type="button"
              disabled={locked}
              onClick={requestDelete}
            >
              删除路由
            </SettingsButton>
          )
        }
      >
        <SettingsButton
          type="button"
          disabled={locked}
          onClick={props.onCancel}
        >
          取消
        </SettingsButton>
        <SettingsButton
          variant="primary"
          type="button"
          disabled={locked || !dirty}
          onClick={save}
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
          ) : null}
          保存
        </SettingsButton>
      </SettingsFooter>
      {confirmation ? (
        <SettingsConfirmDialog
          confirmation={confirmation}
          onCancel={() => setSettingsConfirmation(null)}
        />
      ) : null}
    </section>
  );
}

const reachabilityPresentation: Record<
  ReachabilityResult["status"],
  { tone: SettingsTone; label: string }
> = {
  reachable: { tone: "success", label: "可达" },
  slow: { tone: "warning", label: "较慢" },
  path_not_found: {
    tone: "warning",
    label: "服务器可达，推理路径可能不正确",
  },
  unreachable: { tone: "danger", label: "不可达" },
};
