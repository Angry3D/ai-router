import type { RouteHealthDto } from "../../generated";

export interface RouteHealthPresentation {
  marker: string | null;
  detail: string | null;
  tone: "warning" | "accent";
}

function remainingSeconds(seconds: number, elapsedSeconds: number) {
  return Math.max(0, Math.ceil(seconds) - elapsedSeconds);
}

export function routeHealthPresentation(
  health: RouteHealthDto | null,
  active: boolean,
  elapsedSeconds = 0,
): RouteHealthPresentation {
  if (!health) {
    return {
      marker: active ? "当前" : null,
      detail: null,
      tone: "accent",
    };
  }

  switch (health.kind) {
    case "striking":
      return {
        marker: active
          ? `当前 · ${health.failureCount}/5`
          : `失败 ${health.failureCount}/5`,
        detail: `已累计失败 ${health.failureCount}/5 · 仍使用当前路由，不切换`,
        tone: "warning",
      };
    case "switching":
      return {
        marker: active ? "当前 · 切换中" : "切换中",
        detail: "已达到 5/5 · 正在切换到可用的后续路由",
        tone: "warning",
      };
    case "switch_pending": {
      const remaining =
        health.retryAfterSeconds === null
          ? null
          : remainingSeconds(health.retryAfterSeconds, elapsedSeconds);
      return {
        marker: active ? "当前 · 待切换" : "待切换",
        detail:
          remaining !== null && remaining > 0
            ? `已达到 5/5 · ${remaining} 秒后重试切换，仍使用当前路由`
            : "已达到 5/5 · 等待可用的后续路由",
        tone: "warning",
      };
    }
    case "open": {
      const remaining = remainingSeconds(
        health.retryAfterSeconds,
        elapsedSeconds,
      );
      if (health.recoverySuccesses > 0) {
        return {
          marker: "恢复 1/2",
          detail:
            remaining > 0
              ? `已通过 1/2 次恢复验证 · ${remaining} 秒后再次验证`
              : "已通过 1/2 次恢复验证 · 等待下一个兼容请求",
          tone: "warning",
        };
      }
      if (remaining > 0) {
        return health.origin === "model_bypassed"
          ? {
              marker: `待验证 · ${remaining}s`,
              detail: `曾因模型不兼容被跳过 · ${remaining} 秒后等待兼容请求验证`,
              tone: "warning",
            }
          : {
              marker: `暂停 · ${remaining}s`,
              detail: `暂停使用 · ${remaining} 秒后可恢复验证`,
              tone: "warning",
            };
      }
      return {
        marker: "待验证",
        detail: "已可恢复验证 · 等待下一个兼容请求",
        tone: "warning",
      };
    }
    case "probing":
      return {
        marker: "验证中",
        detail: `正在进行第 ${health.recoverySuccesses > 0 ? 2 : 1}/2 次恢复验证`,
        tone: "accent",
      };
  }
}

export function routeHealthCountdownSeconds(health: RouteHealthDto | null) {
  if (health?.kind === "open") return health.retryAfterSeconds;
  if (health?.kind === "switch_pending") {
    return health.retryAfterSeconds ?? 0;
  }
  return 0;
}
