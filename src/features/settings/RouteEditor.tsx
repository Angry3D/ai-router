import {
  Eye,
  EyeOff,
  FileCode2,
  LoaderCircle,
  Plus,
  Route,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  checkRouteReachability,
  deleteRoute,
  fetchRouteModels,
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
  IpcErrorDto,
  ReachabilityResult,
  RouteEditDto,
  RouteId,
  RouteModelsErrorCategory,
  RouteProtocol,
  RouteSaveInputDto,
} from "../../generated";
import type { RoutesEditorMessages } from "../language/catalog/routes";
import type { LanguageMessages } from "../language/languageMessages";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import { AppScrollArea } from "../shared/AppScrollArea";
import { previewBaseUrl } from "./baseUrlPreview";
import customBalanceScriptScaffold from "./customBalanceScriptScaffold.txt?raw";
import { formatBalanceScript } from "./formatBalanceScript";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsCombobox,
  SettingsConfirmDialog,
  SettingsDivider,
  SettingsFieldRow,
  SettingsFooter,
  SettingsHelpTooltip,
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

function validateCodexModels(
  rows: CodexModelDraft[],
  messages: RoutesEditorMessages,
): CodexModelErrors {
  const errors: CodexModelErrors = {};
  const ids = new Map<string, string>();
  for (const row of rows) {
    const rowErrors: Partial<Record<CodexModelField, string>> = {};
    const modelId = row.modelId.trim();
    if (!modelId) rowErrors.modelId = messages.validationModelIdRequired;
    else if (containsControlCharacter(modelId)) {
      rowErrors.modelId = messages.validationControlCharacter;
    } else if (ids.has(modelId)) {
      rowErrors.modelId = messages.validationModelIdDuplicate;
    } else ids.set(modelId, row.key);
    if (containsControlCharacter(row.displayName.trim())) {
      rowErrors.displayName = messages.validationControlCharacter;
    }
    if (row.contextWindow.trim()) {
      const contextWindow = Number(row.contextWindow);
      if (!Number.isSafeInteger(contextWindow) || contextWindow <= 0) {
        rowErrors.contextWindow = messages.validationPositiveInteger;
      }
    }
    if (Object.keys(rowErrors).length > 0) errors[row.key] = rowErrors;
  }
  return errors;
}

function modelActivationMessage(
  activation: string,
  messages: RoutesEditorMessages,
) {
  switch (activation) {
    case "restart_codex":
      return messages.activationRestartCodex;
    case "connect_codex":
      return messages.activationConnectCodex;
    case "reconnect_codex":
      return messages.activationReconnectCodex;
    case "fix_codex_config":
      return messages.activationFixCodexConfig;
    default:
      return messages.activationSaved;
  }
}

interface RouteFormState {
  name: string;
  baseUrl: string;
  protocol: RouteProtocol;
  apiKey: string;
  menuVisible: boolean;
  queryMode: BalanceQueryMode;
  queryEnabled: boolean;
  customSource: string;
  fallbackExcludedModels: string[];
  models: CodexModelDraft[];
}

export interface RouteEditorHealthDetail {
  text: string;
  tone: "warning" | "accent";
}

type BalanceTestFeedback =
  | { kind: "result"; result: BalanceResult }
  | { kind: "error"; error: IpcErrorDto };

type ModelsFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; models: string[] }
  | { kind: "categoryError"; category: RouteModelsErrorCategory }
  | { kind: "ipcError"; error: IpcErrorDto };

type EditorNotice = "retryRequired" | "scriptFormatFailed" | "systemProxy";

type EditorError =
  | { kind: "ipc"; error: IpcErrorDto }
  | { kind: "notice"; notice: EditorNotice };

function editorNoticeText(
  messages: RoutesEditorMessages,
  notice: EditorNotice,
): string {
  switch (notice) {
    case "retryRequired":
      return messages.saveRetryRequired;
    case "scriptFormatFailed":
      return messages.scriptFormatFailed;
    case "systemProxy":
      return messages.modelsError.system_proxy;
  }
}

function modelsFetchStatusFor(
  state: ModelsFetchState,
  messages: LanguageMessages,
): {
  text: string;
  tone: SettingsTone;
  alert: boolean;
} {
  const editor = messages.routes.editor;
  switch (state.kind) {
    case "loading":
      return { text: editor.modelsFetching, tone: "neutral", alert: false };
    case "ready":
      return state.models.length === 0
        ? { text: editor.modelsEmpty, tone: "warning", alert: false }
        : {
            text: editor.modelsFetched(state.models.length),
            tone: "neutral",
            alert: false,
          };
    case "categoryError":
      return {
        text: editor.modelsError[state.category],
        tone: "danger",
        alert: true,
      };
    case "ipcError":
      return {
        text: ipcErrorMessage(state.error, messages),
        tone: "danger",
        alert: true,
      };
    default:
      return {
        text: editor.modelsNotFetched,
        tone: "neutral",
        alert: false,
      };
  }
}

const emptyRouteForm: RouteFormState = {
  name: "",
  baseUrl: "",
  protocol: "responses",
  apiKey: "",
  menuVisible: true,
  queryMode: "general_v1",
  queryEnabled: false,
  customSource: "",
  fallbackExcludedModels: [],
  models: [],
};

function formFromEdit(edit: RouteEditDto): RouteFormState {
  return {
    name: edit.name,
    baseUrl: edit.baseUrl,
    protocol: edit.protocol,
    apiKey: edit.apiKey,
    menuVisible: edit.menuVisible !== false,
    queryMode: edit.balanceQuery?.mode ?? "general_v1",
    queryEnabled: edit.balanceQuery?.enabled ?? false,
    customSource: edit.balanceQuery?.customSource ?? "",
    fallbackExcludedModels: edit.fallbackExcludedModels ?? [],
    models: codexModelDrafts(edit.models),
  };
}

export function RouteEditor(props: {
  routeId: RouteId | null;
  newRoute: boolean;
  activeRouteId: RouteId | null;
  riskConfirmed: boolean;
  externalBusy: boolean;
  healthDetail?: RouteEditorHealthDetail | null;
  onDirtyChange: (dirty: boolean) => void;
  onCancel: () => void;
  onSaved: (routeId: RouteId) => void;
  onDeleted: (routeId: RouteId) => void;
}) {
  const [edit, setEdit] = useState<RouteEditDto | null>(null);
  const [loadState, setLoadState] = useState<"loading" | "ready" | "error">(
    props.newRoute || props.routeId === null ? "ready" : "loading",
  );
  const { messages } = useLanguage();
  const editorMessages = messages.routes.editor;

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
        <h2>{editorMessages.emptyTitle}</h2>
      </section>
    );
  }
  if (!props.newRoute && loadState === "loading")
    return (
      <section className="route-editor-empty">{editorMessages.loading}</section>
    );
  if (!props.newRoute && (loadState === "error" || edit === null))
    return (
      <section className="route-editor-empty settings-error">
        {editorMessages.loadFailed}
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
  healthDetail?: RouteEditorHealthDetail | null;
  initial: RouteFormState;
  onDirtyChange: (dirty: boolean) => void;
  onCancel: () => void;
  onSaved: (routeId: RouteId) => void;
  onDeleted: (routeId: RouteId) => void;
}) {
  const { onDirtyChange } = props;
  const { messages } = useLanguage();
  const editorMessages = messages.routes.editor;
  const queryClient = useQueryClient();
  const [form, setForm] = useState<RouteFormState>(() => props.initial);
  const [baseline, setBaseline] = useState<RouteFormState>(() => props.initial);
  const [showKey, setShowKey] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<EditorError | null>(null);
  const [reachability, setReachability] = useState<ReachabilityResult | null>(
    null,
  );
  const [balanceFeedback, setBalanceFeedback] =
    useState<BalanceTestFeedback | null>(null);
  const [confirmation, setSettingsConfirmation] =
    useState<SettingsConfirmation | null>(null);
  const [modelErrors, setModelErrors] = useState<CodexModelErrors>({});
  const [fallbackModelInput, setFallbackModelInput] = useState("");
  const [fallbackModelError, setFallbackModelError] = useState<string | null>(
    null,
  );
  const [modelsFetch, setModelsFetch] = useState<ModelsFetchState>({
    kind: "idle",
  });
  const [fallbackPopupHost, setFallbackPopupHost] =
    useState<HTMLElement | null>(null);
  const [modelIdInput, setModelIdInput] = useState("");
  const [retryToken, setRetryToken] = useState<string | null>(null);
  const [modelActivation, setModelActivation] = useState<string | null>(null);
  const nextModelKey = useRef(props.initial.models.length);
  const fallbackModelInputRef = useRef<HTMLInputElement>(null);
  const modelIdComboboxRef = useRef<HTMLInputElement>(null);
  const probeGeneration = useRef(0);
  const modelsGeneration = useRef(0);
  const dirty =
    retryToken !== null ||
    fallbackModelInput.trim().length > 0 ||
    JSON.stringify(form) !== JSON.stringify(baseline);
  const modelsDirty =
    retryToken !== null ||
    JSON.stringify(form.models) !== JSON.stringify(baseline.models);
  const fallbackModelsDirty =
    fallbackModelInput.trim().length > 0 ||
    JSON.stringify(form.fallbackExcludedModels) !==
      JSON.stringify(baseline.fallbackExcludedModels);
  const locked = busy || props.externalBusy;

  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);

  const baseUrlPreview = useMemo(
    () => previewBaseUrl(form.baseUrl, form.protocol),
    [form.baseUrl, form.protocol],
  );
  const inferenceUrl = form.baseUrl.trim()
    ? baseUrlPreview.valid
      ? baseUrlPreview.inferenceUrl
      : editorMessages.invalidUrl
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

  const fetchedModels = modelsFetch.kind === "ready" ? modelsFetch.models : [];
  const customModelOptions = fetchedModels.map((id) => ({
    id,
    disabled: form.models.some((row) => row.modelId.trim() === id),
  }));
  const fallbackModelOptions = fetchedModels.map((id) => ({
    id,
    disabled: form.fallbackExcludedModels.includes(id),
  }));
  const modelsFetchStatus = modelsFetchStatusFor(modelsFetch, messages);

  const patchForm = <K extends keyof RouteFormState>(
    key: K,
    value: RouteFormState[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    if (key === "baseUrl" || key === "protocol") {
      probeGeneration.current += 1;
      setReachability(null);
    }
    if (key === "baseUrl" || key === "protocol" || key === "apiKey") {
      modelsGeneration.current += 1;
      setModelsFetch({ kind: "idle" });
    }
    if (
      ["baseUrl", "protocol", "apiKey", "queryMode", "customSource"].includes(
        key,
      )
    ) {
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
    setModelActivation(null);
  };

  const addModelWithId = (value: string) => {
    const modelId = value.trim();
    if (!modelId) return;
    const key = `new-model-${nextModelKey.current++}`;
    setForm((current) => ({
      ...current,
      models: [
        ...current.models,
        { key, modelId, displayName: "", contextWindow: "" },
      ],
    }));
    setModelIdInput("");
    setError(null);
    setModelActivation(null);
    requestAnimationFrame(() => modelIdComboboxRef.current?.focus());
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
    setModelActivation(null);
  };

  const validateFallbackModel = (value: string) => {
    const modelId = value.trim();
    if (!modelId) return null;
    if (containsControlCharacter(modelId)) {
      return editorMessages.validationControlCharacter;
    }
    if (form.fallbackExcludedModels.includes(modelId)) {
      return editorMessages.validationModelIdDuplicate;
    }
    return null;
  };

  const addFallbackModel = (value: string = fallbackModelInput) => {
    const modelId = value.trim();
    if (!modelId) return;
    const validation = validateFallbackModel(modelId);
    if (validation) {
      setFallbackModelError(validation);
      return;
    }
    patchForm("fallbackExcludedModels", [
      ...form.fallbackExcludedModels,
      modelId,
    ]);
    setFallbackModelInput("");
    setFallbackModelError(null);
    requestAnimationFrame(() => fallbackModelInputRef.current?.focus());
  };

  const removeFallbackModel = (modelId: string) => {
    patchForm(
      "fallbackExcludedModels",
      form.fallbackExcludedModels.filter((candidate) => candidate !== modelId),
    );
    setFallbackModelError(null);
    requestAnimationFrame(() => fallbackModelInputRef.current?.focus());
  };

  const commitSave = async (acceptScriptRisk: boolean) => {
    if (locked) return;
    const pendingFallbackModel = fallbackModelInput.trim();
    const fallbackValidation = pendingFallbackModel
      ? validateFallbackModel(pendingFallbackModel)
      : null;
    if (fallbackValidation) {
      setFallbackModelError(fallbackValidation);
      return;
    }
    const fallbackExcludedModels = pendingFallbackModel
      ? [...form.fallbackExcludedModels, pendingFallbackModel]
      : form.fallbackExcludedModels;
    const validation = validateCodexModels(form.models, editorMessages);
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
        protocol: form.protocol,
        apiKey: form.apiKey,
        menuVisible: form.menuVisible,
        balanceQuery,
        acceptScriptRisk,
        fallbackExcludedModels,
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
        setError({ kind: "notice", notice: "retryRequired" });
        return;
      }
      const savedModels = codexModelDrafts(result.catalog.models).map(
        (model, index) => ({
          ...model,
          key: form.models[index]?.key ?? model.key,
        }),
      );
      const canonicalBaseUrl = previewBaseUrl(form.baseUrl, form.protocol);
      const savedForm = {
        ...form,
        baseUrl: canonicalBaseUrl.valid
          ? canonicalBaseUrl.canonicalPrefix
          : form.baseUrl,
        fallbackExcludedModels,
        models: savedModels,
      };
      setForm(savedForm);
      setBaseline(savedForm);
      setFallbackModelInput("");
      setFallbackModelError(null);
      setRetryToken(null);
      setModelActivation(result.catalog.activation);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
        queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
        queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
      ]);
      props.onSaved(result.routeId);
    } catch (reason) {
      const normalized = normalizeIpcError(reason);
      const fieldMessage = ipcErrorMessage(normalized, messages);
      if (normalized.field?.startsWith("fallbackExcludedModels.")) {
        setFallbackModelError(fieldMessage);
      }
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
            [row.key]: { ...current[row.key], [field]: fieldMessage },
          }));
        }
      }
      setError({ kind: "ipc", error: normalized });
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
        title: editorMessages.scriptRiskTitle,
        body: editorMessages.scriptRiskBody,
        confirmLabel: editorMessages.scriptRiskConfirm,
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
      const result = await checkRouteReachability(form.baseUrl, form.protocol);
      if (generation === probeGeneration.current) {
        setReachability(result);
        switch (result.errorCategory) {
          case "system_proxy_read_failed":
          case "system_proxy_invalid":
          case "system_proxy_automatic_unsupported":
            setError({ kind: "notice", notice: "systemProxy" });
            break;
        }
      }
    } catch (reason) {
      if (generation === probeGeneration.current) {
        setError({ kind: "ipc", error: normalizeIpcError(reason) });
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
        error: normalizeIpcError(reason),
      });
    } finally {
      setBusy(false);
    }
  };

  const fetchModels = async () => {
    const generation = ++modelsGeneration.current;
    setBusy(true);
    setError(null);
    setModelsFetch({ kind: "loading" });
    try {
      const result = await fetchRouteModels({
        baseUrl: form.baseUrl,
        apiKey: form.apiKey,
        protocol: form.protocol,
      });
      if (generation !== modelsGeneration.current) return;
      setModelsFetch(
        result.errorCategory
          ? { kind: "categoryError", category: result.errorCategory }
          : { kind: "ready", models: result.models },
      );
    } catch (reason) {
      if (generation !== modelsGeneration.current) return;
      setModelsFetch({ kind: "ipcError", error: normalizeIpcError(reason) });
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
      setError({ kind: "notice", notice: "scriptFormatFailed" });
    } finally {
      setBusy(false);
    }
  };

  const requestDelete = () => {
    if (!props.routeId || locked) return;
    const active = props.routeId === props.activeRouteId;
    setSettingsConfirmation({
      title: active
        ? editorMessages.deleteActiveTitle(form.name)
        : editorMessages.deleteTitle(form.name),
      body: active
        ? editorMessages.deleteActiveBody
        : editorMessages.deleteBody,
      confirmLabel: editorMessages.deleteConfirm,
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
          .catch((reason) =>
            setError({ kind: "ipc", error: normalizeIpcError(reason) }),
          )
          .finally(() => setBusy(false));
      },
    });
  };

  return (
    <section
      className="route-form-pane"
      aria-label={
        props.newRoute
          ? editorMessages.newRouteTitle
          : editorMessages.editRouteLabel(form.name)
      }
    >
      <AppScrollArea
        className="route-form-scroll"
        viewportClassName="route-form-scroll-viewport"
      >
        <fieldset className="route-form-fields" disabled={locked}>
          <SettingsPageTitle
            title={props.newRoute ? editorMessages.newRouteTitle : form.name}
            detail={props.newRoute ? null : props.healthDetail?.text}
            detailTone={props.healthDetail?.tone}
          />
          <SettingsFieldRow
            label={editorMessages.nameLabel}
            htmlFor="route-name"
            required
          >
            <SettingsTextInput
              id="route-name"
              value={form.name}
              maxLength={30}
              aria-required="true"
              onChange={(event) => patchForm("name", event.target.value)}
            />
          </SettingsFieldRow>
          <SettingsFieldRow label={editorMessages.protocolLabel}>
            <div
              className="settings-segments"
              role="radiogroup"
              aria-label={editorMessages.protocolLabel}
            >
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-protocol"
                  value="responses"
                  checked={form.protocol === "responses"}
                  onChange={() => patchForm("protocol", "responses")}
                />
                <span>Responses API</span>
              </label>
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-protocol"
                  value="chat_completions"
                  checked={form.protocol === "chat_completions"}
                  onChange={() => patchForm("protocol", "chat_completions")}
                />
                <span>Chat Completions</span>
              </label>
            </div>
          </SettingsFieldRow>
          <SettingsFieldRow
            label={editorMessages.baseUrlLabel}
            htmlFor="route-base-url"
            required
          >
            <SettingsTextInput
              id="route-base-url"
              value={form.baseUrl}
              aria-required="true"
              onChange={(event) => patchForm("baseUrl", event.target.value)}
              placeholder="https://example.com/v1"
            />
          </SettingsFieldRow>
          <SettingsReadonlyRow label={editorMessages.inferenceUrlLabel}>
            {inferenceUrl || "-"}
          </SettingsReadonlyRow>
          {insecureHttp ? (
            <p className="inline-warning">
              {editorMessages.insecureHttpWarning}
            </p>
          ) : null}
          <SettingsFieldRow
            label={editorMessages.apiKeyLabel}
            htmlFor="route-api-key"
            required
          >
            <span className="secret-input">
              <SettingsTextInput
                id="route-api-key"
                type={showKey ? "text" : "password"}
                value={form.apiKey}
                aria-required="true"
                onChange={(event) => patchForm("apiKey", event.target.value)}
              />
              <SettingsIconButton
                type="button"
                label={
                  showKey
                    ? editorMessages.hideApiKey
                    : editorMessages.showApiKey
                }
                title={
                  showKey ? editorMessages.hideKey : editorMessages.showKey
                }
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
          <SettingsFieldRow label={editorMessages.menuVisibleLabel}>
            <div className="route-fallback-switch-help">
              <SettingsSwitch
                label={editorMessages.menuVisibleLabel}
                checked={form.menuVisible}
                disabled={
                  props.routeId !== null &&
                  props.activeRouteId === props.routeId
                }
                onChange={(event) =>
                  patchForm("menuVisible", event.target.checked)
                }
              />
              <SettingsHelpTooltip label={editorMessages.menuVisibleHelpLabel}>
                {editorMessages.menuVisibleHelpText}
              </SettingsHelpTooltip>
            </div>
          </SettingsFieldRow>
          <SettingsActionGroup className="route-probe-actions">
            <SettingsButton
              type="button"
              disabled={busy || !baseUrlPreview.valid}
              onClick={() => void probe()}
            >
              {editorMessages.checkReachability}
            </SettingsButton>
            {reachability ? (
              <SettingsStatus
                tone={reachabilityTone[reachability.status]}
              >
                {reachabilityLabel(reachability.status, editorMessages)}
                {reachability.ttfbMs !== null
                  ? ` · ${reachability.ttfbMs} ms`
                  : null}
              </SettingsStatus>
            ) : null}
          </SettingsActionGroup>
          <SettingsActionGroup className="route-probe-actions">
            <SettingsButton
              type="button"
              disabled={
                busy ||
                !form.baseUrl ||
                !form.apiKey ||
                modelsFetch.kind === "loading"
              }
              onClick={() => void fetchModels()}
            >
              {modelsFetch.kind === "loading"
                ? editorMessages.fetchingModels
                : editorMessages.fetchModels}
            </SettingsButton>
            <SettingsStatus
              tone={modelsFetchStatus.tone}
              role={modelsFetchStatus.alert ? "alert" : undefined}
            >
              {modelsFetchStatus.text}
            </SettingsStatus>
          </SettingsActionGroup>
          <SettingsDivider />
          <section
            className="route-fallback-model-section"
            aria-labelledby="route-fallback-model-heading"
          >
            <div className="settings-section-heading">
              <h3
                id="route-fallback-model-heading"
                className="settings-section-title"
              >
                {editorMessages.fallbackModelsTitle}
              </h3>
              <SettingsStatus>
                {fallbackModelsDirty
                  ? editorMessages.fallbackModelsCountUnsaved(
                      form.fallbackExcludedModels.length,
                    )
                  : editorMessages.fallbackModelsCount(
                      form.fallbackExcludedModels.length,
                    )}
              </SettingsStatus>
            </div>
            <p className="fallback-model-notice">
              {editorMessages.fallbackModelsNotice}
            </p>
            <div
              className="fallback-model-popup-host"
              ref={setFallbackPopupHost}
            >
              <div
                className="fallback-model-field"
                data-invalid={fallbackModelError ? "true" : undefined}
              >
                <div className="fallback-model-tags" role="list">
                  {form.fallbackExcludedModels.map((modelId) => (
                    <span
                      className="fallback-model-tag"
                      role="listitem"
                      key={modelId}
                    >
                      <span>{modelId}</span>
                      <button
                        type="button"
                        className="fallback-model-remove"
                        aria-label={editorMessages.removeFallbackModelLabel(
                          modelId,
                        )}
                        title={editorMessages.removeFallbackModelTitle(modelId)}
                        onClick={() => removeFallbackModel(modelId)}
                      >
                        <X aria-hidden="true" size={13} />
                      </button>
                    </span>
                  ))}
                </div>
                <SettingsCombobox
                  inputRef={fallbackModelInputRef}
                  value={fallbackModelInput}
                  onChange={(value) => {
                    setFallbackModelInput(value);
                    setFallbackModelError(null);
                    setError(null);
                  }}
                  options={fallbackModelOptions}
                  onSelect={addFallbackModel}
                  onSubmit={addFallbackModel}
                  ariaLabel={editorMessages.fallbackModelComboboxLabel}
                  placeholder={
                    form.fallbackExcludedModels.length === 0
                      ? editorMessages.fallbackModelPlaceholder
                      : undefined
                  }
                  describedBy={
                    fallbackModelError ? "fallback-model-error" : undefined
                  }
                  maxLength={256}
                  invalid={Boolean(fallbackModelError)}
                  popupHost={fallbackPopupHost}
                  placement="below"
                />
                <SettingsIconButton
                  type="button"
                  label={editorMessages.addModelLabel}
                  title={editorMessages.addModelTitle}
                  disabled={!fallbackModelInput.trim()}
                  onClick={() => addFallbackModel()}
                >
                  <Plus aria-hidden="true" size={14} />
                </SettingsIconButton>
              </div>
            </div>
            <div className="fallback-model-error-slot">
              {fallbackModelError ? (
                <span id="fallback-model-error" role="alert">
                  {fallbackModelError}
                </span>
              ) : null}
            </div>
          </section>

          <SettingsDivider />
          <section
            className="route-model-section"
            aria-labelledby="route-model-heading"
          >
            <div className="settings-section-heading">
              <h3 id="route-model-heading" className="settings-section-title">
                {editorMessages.modelsTitle}
              </h3>
              <SettingsStatus
                tone={
                  retryToken
                    ? "danger"
                    : modelActivation
                      ? "success"
                      : "neutral"
                }
              >
                {modelActivation
                  ? modelActivationMessage(modelActivation, editorMessages)
                  : retryToken
                    ? editorMessages.modelsCountRetry(form.models.length)
                    : modelsDirty
                      ? editorMessages.modelsCountUnsaved(form.models.length)
                      : editorMessages.modelsCount(form.models.length)}
              </SettingsStatus>
            </div>
            <p className="codex-model-notice">{editorMessages.modelsNotice}</p>
            {form.models.length === 0 ? (
              <div className="codex-model-empty">
                {editorMessages.modelsEmptyGrid}
              </div>
            ) : (
              <div
                className="codex-model-grid"
                aria-label={editorMessages.modelsGridLabel}
              >
                <div className="codex-model-grid-header" aria-hidden="true">
                  <span>
                    {editorMessages.columnModelId}{" "}
                    <span
                      className="settings-required-marker"
                      aria-hidden="true"
                    />
                  </span>
                  <span>{editorMessages.columnDisplayName}</span>
                  <span>{editorMessages.columnContextWindow}</span>
                  <span />
                </div>
                {form.models.map((row, index) => {
                  const rowErrors = modelErrors[row.key] ?? {};
                  return (
                    <div className="codex-model-row" key={row.key}>
                      <div className="codex-model-field">
                        <SettingsTextInput
                          aria-label={editorMessages.modelIdFieldLabel(
                            index + 1,
                          )}
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
                          aria-label={editorMessages.displayNameFieldLabel(
                            index + 1,
                          )}
                          aria-invalid={Boolean(rowErrors.displayName)}
                          placeholder={editorMessages.displayNamePlaceholder}
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
                          aria-label={editorMessages.contextWindowFieldLabel(
                            index + 1,
                          )}
                          aria-invalid={Boolean(rowErrors.contextWindow)}
                          type="number"
                          min={1}
                          step={1}
                          placeholder="256000"
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
                        label={editorMessages.removeModelLabel(index + 1)}
                        title={editorMessages.removeModelLabel(index + 1)}
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
              <SettingsCombobox
                inputRef={modelIdComboboxRef}
                value={modelIdInput}
                onChange={(value) => {
                  setModelIdInput(value);
                  setError(null);
                }}
                options={customModelOptions}
                onSelect={addModelWithId}
                onSubmit={addModelWithId}
                ariaLabel={editorMessages.modelSearchLabel}
                placeholder={editorMessages.modelSearchLabel}
                placement="above"
                disabled={busy}
              />
              <SettingsButton
                type="button"
                disabled={busy || !modelIdInput.trim()}
                onClick={() => addModelWithId(modelIdInput)}
              >
                {editorMessages.addModelLabel}
              </SettingsButton>
            </div>
          </section>

          <SettingsDivider />
          <h3 className="settings-section-title">
            {editorMessages.balanceTitle}
          </h3>
          <SettingsFieldRow label={editorMessages.queryModeLabel}>
            <div
              className="settings-segments"
              role="radiogroup"
              aria-label={editorMessages.queryModeAriaLabel}
            >
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-balance-query-mode"
                  value="general_v1"
                  checked={form.queryMode === "general_v1"}
                  onChange={() => patchForm("queryMode", "general_v1")}
                />
                <span>{editorMessages.queryModeGeneral}</span>
              </label>
              <label className="settings-segment-option">
                <input
                  type="radio"
                  name="route-balance-query-mode"
                  value="custom_js"
                  checked={form.queryMode === "custom_js"}
                  onChange={() => patchForm("queryMode", "custom_js")}
                />
                <span>{editorMessages.queryModeCustom}</span>
              </label>
            </div>
          </SettingsFieldRow>
          <SettingsSwitch
            label={editorMessages.queryEnabledLabel}
            checked={form.queryEnabled}
            onChange={(event) =>
              patchForm("queryEnabled", event.target.checked)
            }
          />
          {form.queryMode === "custom_js" ? (
            <SettingsTextarea
              aria-label={editorMessages.scriptAriaLabel}
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
                  {editorMessages.formatScript}
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
                  {editorMessages.insertScaffold}
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
                    ? ipcErrorMessage(balanceFeedback.error, messages)
                    : balanceFeedback.result.isValid
                      ? editorMessages.balanceResult(
                          balanceFeedback.result.remaining === null
                            ? "-"
                            : balanceFeedback.result.remaining.toFixed(2),
                          balanceFeedback.result.unit ?? "",
                        )
                      : (balanceFeedback.result.invalidMessage ??
                        editorMessages.balanceInvalidResult)}
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
                {editorMessages.testBalance}
              </SettingsButton>
            </div>
          </SettingsActionGroup>
          {error ? (
            <p className="settings-error" role="alert">
              {error.kind === "ipc"
                ? ipcErrorMessage(error.error, messages)
                : editorNoticeText(editorMessages, error.notice)}
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
              {editorMessages.deleteRoute}
            </SettingsButton>
          )
        }
      >
        <SettingsButton
          type="button"
          disabled={locked}
          onClick={props.onCancel}
        >
          {messages.shell.cancel}
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
          {messages.shell.save}
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

const reachabilityTone: Record<ReachabilityResult["status"], SettingsTone> = {
  reachable: "success",
  slow: "warning",
  path_not_found: "warning",
  unreachable: "danger",
};

function reachabilityLabel(
  status: ReachabilityResult["status"],
  messages: RoutesEditorMessages,
): string {
  switch (status) {
    case "reachable":
      return messages.reachable;
    case "slow":
      return messages.slow;
    case "path_not_found":
      return messages.pathNotFound;
    case "unreachable":
      return messages.unreachable;
  }
}
