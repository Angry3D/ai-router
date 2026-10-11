import { FolderOpen, LoaderCircle, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  clearMcpImages,
  normalizeIpcError,
  openMcpImageDirectory,
  updateImagesGenerationSettings,
  updateMcpImageCapacityThreshold,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { IpcErrorDto, SettingsSnapshotDto } from "../../generated";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import {
  IMAGE_MODEL_PRESETS,
  imageModelTooltipLines,
  isImageModelPreset,
  type ImageModelPreset,
} from "./imageModelProfile";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsDivider,
  SettingsFieldRow,
  SettingsHelpTooltip,
  SettingsReadonlyRow,
  SettingsSelect,
  SettingsSection,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextInput,
  type SettingsConfirmation,
} from "./SettingsPrimitives";

interface ImageGenerationSettingsSectionProps {
  snapshot: SettingsSnapshotDto;
  focusRequested: boolean;
  onFocusConsumed?: () => void;
}

const CUSTOM_IMAGE_MODEL_OPTION = "custom";

type ImageModelDraft =
  | { choice: "preset"; preset: ImageModelPreset }
  | { choice: "custom"; text: string };

function imageModelDraftFromStored(model: string): ImageModelDraft {
  return isImageModelPreset(model)
    ? { choice: "preset", preset: model }
    : { choice: "custom", text: model };
}

export function ImageGenerationSettingsSection({
  snapshot,
  focusRequested,
  onFocusConsumed,
}: ImageGenerationSettingsSectionProps) {
  const queryClient = useQueryClient();
  const { messages } = useLanguage();
  const imageMessages = messages.imagesPricing.imageGeneration;
  const titleRef = useRef<HTMLHeadingElement>(null);
  const [enabled, setEnabled] = useState(snapshot.imagesGeneration.enabled);
  const [routeId, setRouteId] = useState(snapshot.imagesGeneration.routeId);
  const [timeoutDraft, setTimeoutDraft] = useState(
    String(snapshot.imagesGeneration.timeoutSecs),
  );
  const [modelDraft, setModelDraft] = useState<ImageModelDraft>(() =>
    imageModelDraftFromStored(snapshot.imagesGeneration.model),
  );
  const customModelInputRef = useRef<HTMLInputElement>(null);
  const customModelFocusPending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const [capacityDraft, setCapacityDraft] = useState(
    String(snapshot.mcpImageCapacity.thresholdMib),
  );
  const [capacityOperation, setCapacityOperation] = useState<
    "saving" | "opening" | "clearing" | null
  >(null);
  const [capacitySaved, setCapacitySaved] = useState(false);
  const [capacityError, setCapacityError] = useState<IpcErrorDto | null>(null);
  const [clearConfirmation, setClearConfirmation] =
    useState<SettingsConfirmation | null>(null);
  const capacityThreshold = parseImageCapacityThreshold(capacityDraft);
  const capacityThresholdError =
    capacityThreshold === null ? imageMessages.capacityError : null;
  const capacityUnchanged =
    capacityThreshold === snapshot.mcpImageCapacity.thresholdMib;
  const capacityBusy = capacityOperation !== null;

  useEffect(() => {
    // Keep the editable draft aligned when the authoritative threshold changes.
    // This is an external snapshot synchronization, not an interaction update.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCapacityDraft(String(snapshot.mcpImageCapacity.thresholdMib));
    setCapacitySaved(false);
  }, [snapshot.mcpImageCapacity.thresholdMib]);

  useEffect(() => {
    if (!focusRequested || !titleRef.current) return;
    titleRef.current.scrollIntoView?.({ block: "start" });
    titleRef.current.focus({ preventScroll: true });
    onFocusConsumed?.();
  }, [focusRequested, onFocusConsumed]);
  const modelChoice = modelDraft.choice;
  useEffect(() => {
    // The custom input mounts only after the custom model option is chosen, so
    // focus it once the reveal has committed instead of on every draft edit.
    if (modelChoice !== "custom" || !customModelFocusPending.current) return;
    customModelFocusPending.current = false;
    customModelInputRef.current?.focus();
  }, [modelChoice]);
  const selectedRouteExists =
    routeId !== null &&
    snapshot.routes.some((route) => route.routeId === routeId);
  const timeoutSecs = parseImagesGenerationTimeout(timeoutDraft);
  const timeoutError =
    enabled && timeoutSecs === null ? imageMessages.timeoutError : null;
  const effectiveModel =
    modelDraft.choice === "preset" ? modelDraft.preset : modelDraft.text.trim();
  const modelError =
    enabled && modelDraft.choice === "custom" && effectiveModel === ""
      ? imageMessages.modelRequiredError
      : null;
  const [modelTooltipLine, ...profileTooltipLines] = imageModelTooltipLines(
    effectiveModel,
    imageMessages.modelProfile,
  );
  const unchanged =
    enabled === snapshot.imagesGeneration.enabled &&
    routeId === snapshot.imagesGeneration.routeId &&
    timeoutSecs === snapshot.imagesGeneration.timeoutSecs &&
    effectiveModel === snapshot.imagesGeneration.model;
  const persistedRouteExists =
    snapshot.imagesGeneration.routeId !== null &&
    snapshot.routes.some(
      (route) => route.routeId === snapshot.imagesGeneration.routeId,
    );
  const status = !snapshot.imagesGeneration.enabled
    ? { label: imageMessages.statusDisabled, tone: "neutral" as const }
    : persistedRouteExists
      ? { label: imageMessages.statusEnabled, tone: "success" as const }
      : { label: imageMessages.statusRouteRequired, tone: "warning" as const };

  const updateDraft = (next: {
    enabled?: boolean;
    routeId?: string | null;
  }) => {
    if (next.enabled !== undefined) {
      setEnabled(next.enabled);
      if (!next.enabled) {
        setTimeoutDraft(String(snapshot.imagesGeneration.timeoutSecs));
        setModelDraft(
          imageModelDraftFromStored(snapshot.imagesGeneration.model),
        );
      }
    }
    if (next.routeId !== undefined) setRouteId(next.routeId);
    setSaved(false);
    setError(null);
  };

  const selectModel = (value: string) => {
    if (value === CUSTOM_IMAGE_MODEL_OPTION) {
      customModelFocusPending.current = true;
      setModelDraft({ choice: "custom", text: "" });
    } else if (isImageModelPreset(value)) {
      setModelDraft({ choice: "preset", preset: value });
    } else {
      return;
    }
    setSaved(false);
    setError(null);
  };

  const updateCustomModel = (text: string) => {
    setModelDraft({ choice: "custom", text });
    setSaved(false);
    setError(null);
  };

  const apply = async () => {
    if (
      busy ||
      unchanged ||
      timeoutSecs === null ||
      modelError !== null ||
      (enabled && !selectedRouteExists)
    )
      return;
    setBusy(true);
    setSaved(false);
    setError(null);
    try {
      await updateImagesGenerationSettings({
        enabled,
        routeId: selectedRouteExists ? routeId : null,
        timeoutSecs,
        model: effectiveModel,
      });
      setTimeoutDraft(String(timeoutSecs));
      setSaved(true);
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setBusy(false);
    }
  };

  const refreshCapacity = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
    ]);
  };

  const saveCapacityThreshold = async () => {
    if (capacityBusy || capacityUnchanged || capacityThreshold === null) return;
    setCapacityOperation("saving");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await updateMcpImageCapacityThreshold(capacityThreshold);
      setCapacityDraft(String(capacityThreshold));
      setCapacitySaved(true);
      await refreshCapacity();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason));
    } finally {
      setCapacityOperation(null);
    }
  };

  const openImageDirectory = async () => {
    if (capacityBusy) return;
    setCapacityOperation("opening");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await openMcpImageDirectory();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason));
    } finally {
      setCapacityOperation(null);
    }
  };

  const clearImages = async () => {
    if (capacityBusy) return;
    setClearConfirmation(null);
    setCapacityOperation("clearing");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await clearMcpImages();
      await refreshCapacity();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason));
      await refreshCapacity();
    } finally {
      setCapacityOperation(null);
    }
  };

  return (
    <SettingsSection
      title={imageMessages.sectionTitle}
      titleId="codex-image-generation-title"
      titleRef={titleRef}
      titleTabIndex={focusRequested ? -1 : undefined}
      titleAccessory={
        <SettingsHelpTooltip label={imageMessages.tooltipLabel}>
          <strong>{modelTooltipLine}</strong>
          {profileTooltipLines.map((line) => (
            <span key={line}>{line}</span>
          ))}
        </SettingsHelpTooltip>
      }
      status={
        <SettingsStatus tone={status.tone}>{status.label}</SettingsStatus>
      }
    >
      <SettingsFieldRow label={imageMessages.codexToolsLabel}>
        <SettingsSwitch
          label={imageMessages.enableLabel}
          checked={enabled}
          disabled={busy}
          onChange={(event) =>
            updateDraft({ enabled: event.currentTarget.checked })
          }
        />
      </SettingsFieldRow>
      <SettingsFieldRow
        label={imageMessages.routeLabel}
        htmlFor="images-generation-route"
      >
        <SettingsSelect
          id="images-generation-route"
          className="images-generation-route-select"
          aria-label={imageMessages.routeLabel}
          value={selectedRouteExists ? routeId : ""}
          disabled={busy || !enabled}
          onChange={(event) =>
            updateDraft({ routeId: event.currentTarget.value || null })
          }
        >
          <option value="">{imageMessages.routePlaceholder}</option>
          {snapshot.routes.map((route) => (
            <option key={route.routeId} value={route.routeId}>
              {route.name}
            </option>
          ))}
        </SettingsSelect>
      </SettingsFieldRow>
      <SettingsFieldRow
        label={imageMessages.modelLabel}
        htmlFor="images-generation-model"
      >
        <div
          className="images-generation-model-control"
          data-custom={modelDraft.choice === "custom" ? "true" : "false"}
        >
          <SettingsSelect
            id="images-generation-model"
            className="images-generation-model-select"
            aria-label={imageMessages.modelLabel}
            value={
              modelDraft.choice === "custom"
                ? CUSTOM_IMAGE_MODEL_OPTION
                : modelDraft.preset
            }
            disabled={busy || !enabled}
            onChange={(event) => selectModel(event.currentTarget.value)}
          >
            {IMAGE_MODEL_PRESETS.map((preset) => (
              <option key={preset} value={preset}>
                {preset}
              </option>
            ))}
            <option value={CUSTOM_IMAGE_MODEL_OPTION}>
              {imageMessages.modelCustomOption}
            </option>
          </SettingsSelect>
          {modelDraft.choice === "custom" ? (
            <SettingsTextInput
              ref={customModelInputRef}
              id="images-generation-model-custom"
              type="text"
              aria-label={imageMessages.customModelLabel}
              placeholder={imageMessages.customModelPlaceholder}
              autoComplete="off"
              spellCheck={false}
              value={modelDraft.text}
              disabled={busy || !enabled}
              aria-invalid={modelError ? "true" : undefined}
              aria-describedby={
                modelError ? "images-generation-model-error" : undefined
              }
              onChange={(event) => updateCustomModel(event.currentTarget.value)}
            />
          ) : null}
        </div>
      </SettingsFieldRow>
      <SettingsFieldRow
        label={imageMessages.timeoutLabel}
        htmlFor="images-generation-timeout"
      >
        <div className="parameter-field">
          <div className="parameter-input-control images-generation-timeout-control">
            <SettingsTextInput
              id="images-generation-timeout"
              type="number"
              min={600}
              max={3600}
              step={1}
              inputMode="numeric"
              value={timeoutDraft}
              disabled={busy || !enabled}
              aria-invalid={timeoutError ? "true" : undefined}
              aria-describedby={
                timeoutError ? "images-generation-timeout-error" : undefined
              }
              onChange={(event) => {
                setTimeoutDraft(event.currentTarget.value);
                setSaved(false);
                setError(null);
              }}
            />
            <span>{imageMessages.timeoutUnit}</span>
          </div>
          {timeoutError ? (
            <p
              id="images-generation-timeout-error"
              className="parameter-field-error"
              role="alert"
            >
              {timeoutError}
            </p>
          ) : null}
        </div>
      </SettingsFieldRow>
      <SettingsActionGroup>
        <SettingsButton
          type="button"
          variant="primary"
          disabled={
            busy ||
            unchanged ||
            timeoutSecs === null ||
            modelError !== null ||
            (enabled && !selectedRouteExists)
          }
          onClick={() => void apply()}
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
          ) : null}
          {imageMessages.applyButton}
        </SettingsButton>
        {modelError ? (
          <span
            id="images-generation-model-error"
            className="settings-error"
            role="alert"
          >
            {modelError}
          </span>
        ) : null}
        {saved ? (
          <SettingsStatus tone="success">
            {imageMessages.savedStatus}
          </SettingsStatus>
        ) : null}
        {error ? (
          <span className="settings-error" role="alert">
            {ipcErrorMessage(error, messages)}
          </span>
        ) : null}
      </SettingsActionGroup>
      <SettingsDivider />
      <div className="mcp-image-storage-settings">
        <SettingsReadonlyRow label={imageMessages.localImagesLabel}>
          {snapshot.mcpImageCapacity.available
            ? imageMessages.localImageSummary(
                snapshot.mcpImageCapacity.imageCount,
                formatImageBytes(snapshot.mcpImageCapacity.bytes),
              )
            : "—"}
        </SettingsReadonlyRow>
        <SettingsFieldRow
          label={imageMessages.capacityLabel}
          htmlFor="mcp-image-capacity-threshold"
          className="mcp-image-capacity-row"
        >
          <div className="mcp-image-capacity-control">
            <SettingsTextInput
              id="mcp-image-capacity-threshold"
              type="number"
              min={128}
              max={102400}
              step={1}
              inputMode="numeric"
              value={capacityDraft}
              disabled={capacityBusy}
              aria-invalid={capacityThresholdError ? "true" : undefined}
              aria-describedby={
                capacityThresholdError
                  ? "mcp-image-capacity-threshold-error"
                  : undefined
              }
              onChange={(event) => {
                setCapacityDraft(event.currentTarget.value);
                setCapacitySaved(false);
                setCapacityError(null);
              }}
            />
            <span className="mcp-image-capacity-unit">
              {imageMessages.capacityUnit}
            </span>
            <SettingsButton
              type="button"
              disabled={
                capacityBusy ||
                capacityUnchanged ||
                capacityThreshold === null
              }
              onClick={() => void saveCapacityThreshold()}
            >
              {capacityOperation === "saving" ? (
                <LoaderCircle aria-hidden="true" className="spin" size={15} />
              ) : null}
              {messages.shell.save}
            </SettingsButton>
            {capacitySaved ? (
              <SettingsStatus tone="success">
                {imageMessages.savedStatus}
              </SettingsStatus>
            ) : null}
          </div>
          {capacityThresholdError ? (
            <p
              id="mcp-image-capacity-threshold-error"
              className="parameter-field-error"
              role="alert"
            >
              {capacityThresholdError}
            </p>
          ) : null}
        </SettingsFieldRow>
        {snapshot.mcpImageCapacity.overThreshold ? (
          <p className="mcp-image-capacity-warning" role="status">
            {imageMessages.capacityWarning}
          </p>
        ) : null}
        {!snapshot.mcpImageCapacity.available ? (
          <p className="settings-error mcp-image-capacity-message" role="alert">
            {imageMessages.directoryUnavailable}
          </p>
        ) : null}
        {capacityError ? (
          <p className="settings-error mcp-image-capacity-message" role="alert">
            {ipcErrorMessage(capacityError, messages)}
          </p>
        ) : null}
        <SettingsActionGroup className="mcp-image-storage-actions">
          <SettingsButton
            type="button"
            disabled={capacityBusy}
            onClick={() => void openImageDirectory()}
          >
            {capacityOperation === "opening" ? (
              <LoaderCircle aria-hidden="true" className="spin" size={15} />
            ) : (
              <FolderOpen aria-hidden="true" size={16} />
            )}
            {imageMessages.openDirectoryButton}
          </SettingsButton>
          <SettingsButton
            type="button"
            variant="danger"
            disabled={
              capacityBusy ||
              !snapshot.mcpImageCapacity.available ||
              snapshot.mcpImageCapacity.imageCount === 0
            }
            onClick={() =>
              setClearConfirmation({
                title: imageMessages.clearConfirmTitle,
                body: imageMessages.clearConfirmBody,
                details: imageMessages.clearConfirmDetails(
                  snapshot.mcpImageCapacity.imageCount,
                  formatImageBytes(snapshot.mcpImageCapacity.bytes, true),
                ),
                confirmLabel: imageMessages.clearConfirmLabel,
                destructive: true,
                onConfirm: () => void clearImages(),
              })
            }
          >
            {capacityOperation === "clearing" ? (
              <LoaderCircle aria-hidden="true" className="spin" size={15} />
            ) : (
              <Trash2 aria-hidden="true" size={16} />
            )}
            {imageMessages.clearImagesButton}
          </SettingsButton>
        </SettingsActionGroup>
      </div>
      {clearConfirmation ? (
        <SettingsConfirmDialog
          confirmation={clearConfirmation}
          onCancel={() => setClearConfirmation(null)}
        />
      ) : null}
    </SettingsSection>
  );
}

function parseImageCapacityThreshold(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 128 && parsed <= 102400
    ? parsed
    : null;
}

function formatImageBytes(bytes: number, spaced = false): string {
  const separator = spaced ? " " : "";
  const gib = 1024 ** 3;
  const mib = 1024 ** 2;
  if (bytes >= gib) return `${trimDecimal(bytes / gib, 2)}${separator}G${spaced ? "B" : ""}`;
  if (bytes >= mib) return `${trimDecimal(bytes / mib, 1)}${separator}M${spaced ? "B" : ""}`;
  return `${trimDecimal(bytes / 1024, 1)}${separator}K${spaced ? "B" : ""}`;
}

function trimDecimal(value: number, digits: number): string {
  return value.toFixed(digits).replace(/\.0+$|(?<=\.[0-9]*)0+$/, "");
}

function parseImagesGenerationTimeout(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 600 && parsed <= 3600
    ? parsed
    : null;
}
