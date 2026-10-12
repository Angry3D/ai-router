import type { PricingTableDto, PricingTableFailureDto } from "../../generated";
import type { PricingMessages } from "../language/catalog/imagesPricing";
import type { SettingsTone } from "./SettingsPrimitives";

/** Longest page diagnostic the status line shows; the log keeps it in full. */
const MAX_FAILURE_REASON_CHARS = 32;

export interface PricingPresentation {
  sectionLabel: string;
  sectionTone: SettingsTone;
  actionText: string;
  actionTone: SettingsTone;
}

function formatSyncedAt(value: number | null): string {
  if (value === null) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (component: number) => String(component).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function syncedSectionLabel(
  snapshot: PricingTableDto,
  messages: PricingMessages,
): string {
  if (snapshot.localState !== "loaded") return messages.statusUnsynced;
  const syncedAt = formatSyncedAt(snapshot.syncedAtMs);
  return syncedAt ? messages.statusSyncedAt(syncedAt) : messages.statusSynced;
}

/** Copy for a failed run; the page diagnostic is bounded for the status line. */
function failureActionText(
  failure: PricingTableFailureDto | null,
  messages: PricingMessages,
): string {
  if (failure?.kind === "store") {
    return messages.failureStore;
  }
  if (!failure || failure.kind !== "page") {
    return messages.failureTransport;
  }
  const reason = failure.reason?.trim() ?? "";
  if (!reason) return messages.failurePageStructure;
  // Clamping by character keeps a surrogate pair from being split.
  const characters = Array.from(reason);
  const shown =
    characters.length > MAX_FAILURE_REASON_CHARS
      ? `${characters.slice(0, MAX_FAILURE_REASON_CHARS - 1).join("")}…`
      : reason;
  return messages.failurePageStructureWithReason(shown);
}

export function pricingPresentation(
  snapshot: PricingTableDto | null,
  messages: PricingMessages,
  syncing: boolean = snapshot?.status === "syncing",
): PricingPresentation {
  const sectionLabel = snapshot
    ? syncedSectionLabel(snapshot, messages)
    : messages.statusUnsynced;
  if (syncing) {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: messages.actionSyncing,
      actionTone: "neutral",
    };
  }
  if (!snapshot) {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: messages.actionNotSynced,
      actionTone: "neutral",
    };
  }
  if (snapshot.status === "error") {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: failureActionText(snapshot.failure, messages),
      actionTone: "danger",
    };
  }
  if (snapshot.localState === "loaded") {
    return {
      sectionLabel,
      sectionTone: "success",
      actionText: "",
      actionTone: "neutral",
    };
  }
  if (snapshot.localState === "corrupt") {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: messages.actionCorrupt,
      actionTone: "neutral",
    };
  }
  return {
    sectionLabel,
    sectionTone: "neutral",
    actionText: messages.actionNotSynced,
    actionTone: "neutral",
  };
}
