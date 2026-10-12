import type { AppLocale } from "../../../generated";

/**
 * Image-model profile copy mirrored from `ImageModelProfile::for_model` in
 * `crates/router-core/src/proxy/images.rs`. Only display text lives here; the
 * profile classification itself stays in `imageModelProfile.ts`.
 */
export interface ImageModelProfileMessages {
  /** Names the effective model; the caller passes `—` for an empty draft. */
  modelTooltipTitle: (model: string) => string;
  png: string;
  flexibleSize: string;
  flexibleExperimental: string;
  standardSize: string;
  baseQuality: string;
  extendedQuality: string;
}

/** Codex image-generation settings plus local image storage management. */
export interface ImageGenerationMessages {
  sectionTitle: string;
  tooltipLabel: string;
  statusDisabled: string;
  statusEnabled: string;
  statusRouteRequired: string;
  codexToolsLabel: string;
  enableLabel: string;
  routeLabel: string;
  routePlaceholder: string;
  modelLabel: string;
  modelCustomOption: string;
  customModelLabel: string;
  customModelPlaceholder: string;
  timeoutLabel: string;
  timeoutUnit: string;
  applyButton: string;
  savedStatus: string;
  timeoutError: string;
  modelRequiredError: string;
  modelProfile: ImageModelProfileMessages;
  localImagesLabel: string;
  localImageSummary: (count: number, size: string) => string;
  capacityLabel: string;
  capacityUnit: string;
  capacityError: string;
  capacityWarning: string;
  directoryUnavailable: string;
  openDirectoryButton: string;
  clearImagesButton: string;
  clearConfirmTitle: string;
  clearConfirmBody: string;
  clearConfirmDetails: (count: number, size: string) => string;
  clearConfirmLabel: string;
}

/** Pricing table section: sync state, failure copy and the read-only table. */
export interface PricingMessages {
  sectionTitle: string;
  syncButton: string;
  statusUnsynced: string;
  statusSynced: string;
  statusSyncedAt: (timestamp: string) => string;
  actionSyncing: string;
  actionNotSynced: string;
  actionCorrupt: string;
  failureStore: string;
  failureTransport: string;
  failurePageStructure: string;
  /** `reason` arrives already clamped to the status-line width. */
  failurePageStructureWithReason: (reason: string) => string;
  captionBilling: string;
  captionSourcePrefix: string;
  sourceLinkLabel: string;
  captionNote: string;
  tableLabel: string;
  columnModel: string;
  columnContext: string;
  columnInput: string;
  columnCachedRead: string;
  columnCachedWrite: string;
  columnOutput: string;
  columnSource: string;
  bandShort: string;
  bandLong: string;
  sourceOfficial: string;
  sourceBundled: string;
}

/** Image-generation settings and pricing-table surfaces. */
export interface ImagesPricingMessages {
  imageGeneration: ImageGenerationMessages;
  pricing: PricingMessages;
}

export const imagesPricingMessages: Record<AppLocale, ImagesPricingMessages> = {
  "zh-CN": {
    imageGeneration: {
      sectionTitle: "图片生成",
      tooltipLabel: "图片生成说明",
      statusDisabled: "未启用",
      statusEnabled: "已启用",
      statusRouteRequired: "需要选择路由",
      codexToolsLabel: "Codex 图片工具",
      enableLabel: "启用",
      routeLabel: "图片路由",
      routePlaceholder: "选择路由",
      modelLabel: "生图模型",
      modelCustomOption: "自定义",
      customModelLabel: "自定义生图模型",
      customModelPlaceholder: "输入模型 ID",
      timeoutLabel: "生成等待上限",
      timeoutUnit: "秒",
      applyButton: "应用",
      savedStatus: "已保存",
      timeoutError: "请输入 600 至 3600 的整数。",
      modelRequiredError: "请输入模型 ID。",
      modelProfile: {
        modelTooltipTitle: (model) => `模型：${model}`,
        png: "每次生成一张 PNG。",
        flexibleSize:
          "尺寸支持 auto 或 宽x高；两条边都是 16 的倍数，最长边不超过 3,840px，比例不超过 3:1，总像素为 655,360–8,294,400。",
        flexibleExperimental:
          "超过 3,686,400 像素属于实验性范围。常用尺寸：1024x1024、1536x1024、1024x1536、2048x1152、3840x2160。",
        standardSize: "尺寸仅支持 auto、1024x1024、1536x1024、1024x1536。",
        baseQuality: "质量：low、medium、high、auto。",
        extendedQuality: "质量：low、medium、high、xhigh、max、auto。",
      },
      localImagesLabel: "本地图片",
      localImageSummary: (count, size) => `${count}张（${size}）`,
      capacityLabel: "容量提醒",
      capacityUnit: "MB",
      capacityError: "请输入 128 至 102400 的整数。",
      capacityWarning: "已达到容量提醒阈值，生成图片仍可继续使用。",
      directoryUnavailable: "图片目录暂时无法读取。",
      openDirectoryButton: "打开图片目录",
      clearImagesButton: "清除生成图片",
      clearConfirmTitle: "清除生成图片？",
      clearConfirmBody:
        "这些图片会被永久删除。历史任务中仅保存在此目录的图片可能无法再预览、处理或复用。",
      clearConfirmDetails: (count, size) =>
        `将清除 ${count} 张图片，占用 ${size}。`,
      clearConfirmLabel: "清除图片",
    },
    pricing: {
      sectionTitle: "模型价格",
      syncButton: "同步官网",
      statusUnsynced: "未同步",
      statusSynced: "已同步",
      statusSyncedAt: (timestamp) => `已同步 · ${timestamp}`,
      actionSyncing: "正在访问官网…",
      actionNotSynced: "尚未同步，当前使用内置价格。",
      actionCorrupt: "本地价格表不可用，已回退内置",
      failureStore: "同步失败：本地价格表写入失败，已保留上次数据。",
      failureTransport: "同步失败：官网暂时无法访问，已保留上次数据。",
      failurePageStructure: "同步失败：官网页面结构变化，已保留上次数据。",
      failurePageStructureWithReason: (reason) =>
        `同步失败：官网页面结构变化（${reason}），已保留上次数据。`,
      captionBilling:
        "美元 / 每 100 万 tokens。输入 ≤27.2 万 tokens 按「短」计价，超出整单按「长」。",
      captionSourcePrefix: "数据来源：",
      sourceLinkLabel: "打开 OpenAI 官方定价页",
      captionNote: "（仅 GPT 模型随官网同步）",
      tableLabel: "模型价格表",
      columnModel: "模型",
      columnContext: "上下文",
      columnInput: "输入",
      columnCachedRead: "缓存读",
      columnCachedWrite: "缓存写",
      columnOutput: "输出",
      columnSource: "来源",
      bandShort: "短",
      bandLong: "长",
      sourceOfficial: "官网",
      sourceBundled: "内置",
    },
  },
  en: {
    imageGeneration: {
      sectionTitle: "Image generation",
      tooltipLabel: "About image generation",
      statusDisabled: "Disabled",
      statusEnabled: "Enabled",
      statusRouteRequired: "Route required",
      codexToolsLabel: "Codex image tools",
      enableLabel: "Enable",
      routeLabel: "Image route",
      routePlaceholder: "Select route",
      modelLabel: "Image model",
      modelCustomOption: "Custom",
      customModelLabel: "Custom image model",
      customModelPlaceholder: "Enter model ID",
      timeoutLabel: "Generation timeout",
      timeoutUnit: "sec",
      applyButton: "Apply",
      savedStatus: "Saved",
      timeoutError: "Enter an integer between 600 and 3600.",
      modelRequiredError: "Enter a model ID.",
      modelProfile: {
        modelTooltipTitle: (model) => `Model: ${model}`,
        png: "Each generation produces one PNG.",
        flexibleSize:
          "Size accepts auto or WxH; both edges must be multiples of 16, the longest edge at most 3,840px, the aspect ratio at most 3:1, and the total pixel count between 655,360 and 8,294,400.",
        flexibleExperimental:
          "Above 3,686,400 pixels is experimental. Common sizes: 1024x1024, 1536x1024, 1024x1536, 2048x1152, 3840x2160.",
        standardSize: "Size supports only auto, 1024x1024, 1536x1024, 1024x1536.",
        baseQuality: "Quality: low, medium, high, auto.",
        extendedQuality: "Quality: low, medium, high, xhigh, max, auto.",
      },
      localImagesLabel: "Local images",
      localImageSummary: (count, size) =>
        `${count} image${count === 1 ? "" : "s"} (${size})`,
      capacityLabel: "Capacity alert",
      capacityUnit: "MB",
      capacityError: "Enter an integer between 128 and 102400.",
      capacityWarning:
        "The capacity alert threshold has been reached; image generation can continue.",
      directoryUnavailable: "The image directory is temporarily unreadable.",
      openDirectoryButton: "Open image directory",
      clearImagesButton: "Clear generated images",
      clearConfirmTitle: "Clear generated images?",
      clearConfirmBody:
        "These images are permanently deleted. Images kept only in this directory by past tasks may no longer be previewable, processable, or reusable.",
      clearConfirmDetails: (count, size) =>
        `This clears ${count} image${count === 1 ? "" : "s"}, using ${size}.`,
      clearConfirmLabel: "Clear images",
    },
    pricing: {
      sectionTitle: "Model pricing",
      syncButton: "Sync from website",
      statusUnsynced: "Not synced",
      statusSynced: "Synced",
      statusSyncedAt: (timestamp) => `Synced · ${timestamp}`,
      actionSyncing: "Contacting the website…",
      actionNotSynced: "Not synced yet; using built-in prices.",
      actionCorrupt: "Local price table unavailable; using built-in prices",
      failureStore:
        "Sync failed: could not write the local price table; previous data kept.",
      failureTransport:
        "Sync failed: the website is temporarily unreachable; previous data kept.",
      failurePageStructure:
        "Sync failed: the website page structure changed; previous data kept.",
      failurePageStructureWithReason: (reason) =>
        `Sync failed: the website page structure changed (${reason}); previous data kept.`,
      captionBilling:
        "USD per 1M tokens. Requests with ≤272K input tokens are billed as “short”; anything above is billed as “long” for the whole request.",
      captionSourcePrefix: "Source: ",
      sourceLinkLabel: "Open the OpenAI pricing page",
      captionNote: "(only GPT models sync with the website)",
      tableLabel: "Model pricing table",
      columnModel: "Model",
      columnContext: "Context",
      columnInput: "Input",
      columnCachedRead: "Cached read",
      columnCachedWrite: "Cached write",
      columnOutput: "Output",
      columnSource: "Source",
      bandShort: "Short",
      bandLong: "Long",
      sourceOfficial: "Website",
      sourceBundled: "Built-in",
    },
  },
};
