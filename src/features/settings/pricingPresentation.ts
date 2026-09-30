import type { PricingTableDto, PricingTableFailureDto } from "../../generated";
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

function syncedSectionLabel(snapshot: PricingTableDto): string {
  if (snapshot.localState !== "loaded") return "未同步";
  const syncedAt = formatSyncedAt(snapshot.syncedAtMs);
  return syncedAt ? `已同步 · ${syncedAt}` : "已同步";
}

/** Copy for a failed run; the page diagnostic is bounded for the status line. */
function failureActionText(failure: PricingTableFailureDto | null): string {
  if (failure?.kind === "store") {
    return "同步失败：本地价格表写入失败，已保留上次数据。";
  }
  if (!failure || failure.kind !== "page") {
    return "同步失败：官网暂时无法访问，已保留上次数据。";
  }
  const reason = failure.reason?.trim() ?? "";
  if (!reason) return "同步失败：官网页面结构变化，已保留上次数据。";
  // Clamping by character keeps a surrogate pair from being split.
  const characters = Array.from(reason);
  const shown =
    characters.length > MAX_FAILURE_REASON_CHARS
      ? `${characters.slice(0, MAX_FAILURE_REASON_CHARS - 1).join("")}…`
      : reason;
  return `同步失败：官网页面结构变化（${shown}），已保留上次数据。`;
}

export function pricingPresentation(
  snapshot: PricingTableDto | null,
  syncing: boolean = snapshot?.status === "syncing",
): PricingPresentation {
  const sectionLabel = snapshot ? syncedSectionLabel(snapshot) : "未同步";
  if (syncing) {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: "正在访问官网…",
      actionTone: "neutral",
    };
  }
  if (!snapshot) {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: "尚未同步，当前使用内置价格。",
      actionTone: "neutral",
    };
  }
  if (snapshot.status === "error") {
    return {
      sectionLabel,
      sectionTone: "neutral",
      actionText: failureActionText(snapshot.failure),
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
      actionText: "本地价格表不可用，已回退内置",
      actionTone: "neutral",
    };
  }
  return {
    sectionLabel,
    sectionTone: "neutral",
    actionText: "尚未同步，当前使用内置价格。",
    actionTone: "neutral",
  };
}
