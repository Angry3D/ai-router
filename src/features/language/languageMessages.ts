import type { AppLocale } from "../../generated";

import { bootstrapMessages, type BootstrapMessages } from "./catalog/bootstrap";
import { codexMessages, type CodexMessages } from "./catalog/codex";
import { errorMessages, type ErrorMessages } from "./catalog/errors";
import {
  imagesPricingMessages,
  type ImagesPricingMessages,
} from "./catalog/imagesPricing";
import { menuMessages, type MenuMessages } from "./catalog/menu";
import { routesMessages, type RoutesMessages } from "./catalog/routes";
import { sharedMessages, type SharedMessages } from "./catalog/shared";
import { shellMessages, type ShellMessages } from "./catalog/shell";
import {
  systemRecoveryMessages,
  type SystemRecoveryMessages,
} from "./catalog/systemRecovery";
import { updatesMessages, type UpdatesMessages } from "./catalog/updates";
import { usageMessages, type UsageMessages } from "./catalog/usage";

/**
 * The one catalog contract: complete per-feature sections with identical
 * keys and function signatures in both locales. Feature sections are owned
 * by their group; `shared` and `errors` are integration-owned.
 */
export interface LanguageMessages {
  shared: SharedMessages;
  shell: ShellMessages;
  menu: MenuMessages;
  routes: RoutesMessages;
  codex: CodexMessages;
  imagesPricing: ImagesPricingMessages;
  systemRecovery: SystemRecoveryMessages;
  updates: UpdatesMessages;
  usage: UsageMessages;
  bootstrap: BootstrapMessages;
  errors: ErrorMessages;
}

export const languageMessages: Record<AppLocale, LanguageMessages> = {
  "zh-CN": {
    shared: sharedMessages["zh-CN"],
    shell: shellMessages["zh-CN"],
    menu: menuMessages["zh-CN"],
    routes: routesMessages["zh-CN"],
    codex: codexMessages["zh-CN"],
    imagesPricing: imagesPricingMessages["zh-CN"],
    systemRecovery: systemRecoveryMessages["zh-CN"],
    updates: updatesMessages["zh-CN"],
    usage: usageMessages["zh-CN"],
    bootstrap: bootstrapMessages["zh-CN"],
    errors: errorMessages["zh-CN"],
  },
  en: {
    shared: sharedMessages.en,
    shell: shellMessages.en,
    menu: menuMessages.en,
    routes: routesMessages.en,
    codex: codexMessages.en,
    imagesPricing: imagesPricingMessages.en,
    systemRecovery: systemRecoveryMessages.en,
    updates: updatesMessages.en,
    usage: usageMessages.en,
    bootstrap: bootstrapMessages.en,
    errors: errorMessages.en,
  },
};
