import { describe, expect, it } from "vitest";

import { routeHealthPresentation } from "./routeHealthPresentation";

describe("routeHealthPresentation", () => {
  it("maps the complete route health state family without displaying zero seconds", () => {
    expect(
      routeHealthPresentation({ kind: "striking", failureCount: 3 }, true),
    ).toEqual({
      marker: "当前 · 3/5",
      detail: "已累计失败 3/5 · 仍使用当前路由，不切换",
      tone: "warning",
    });
    expect(routeHealthPresentation({ kind: "switching" }, true).marker).toBe(
      "当前 · 切换中",
    );
    expect(
      routeHealthPresentation(
        { kind: "switch_pending", retryAfterSeconds: 20 },
        true,
        20,
      ).detail,
    ).toBe("已达到 5/5 · 等待可用的后续路由");
    expect(
      routeHealthPresentation(
        {
          kind: "open",
          origin: "provider_failure",
          recoverySuccesses: 0,
          retryAfterSeconds: 60,
        },
        false,
        60,
      ),
    ).toMatchObject({
      marker: "待验证",
      detail: "已可恢复验证 · 等待下一个兼容请求",
    });
    expect(
      routeHealthPresentation(
        {
          kind: "open",
          origin: "model_bypassed",
          recoverySuccesses: 1,
          retryAfterSeconds: 120,
        },
        false,
      ).marker,
    ).toBe("恢复 1/2");
    expect(
      routeHealthPresentation({ kind: "probing", recoverySuccesses: 1 }, false),
    ).toEqual({
      marker: "验证中",
      detail: "正在进行第 2/2 次恢复验证",
      tone: "accent",
    });
  });
});
