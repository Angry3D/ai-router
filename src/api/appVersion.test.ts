import { beforeEach, describe, expect, it, vi } from "vitest";

import { getRunningAppVersion } from "./appVersion";

const app = vi.hoisted(() => ({
  getVersion: vi.fn(),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: app.getVersion,
}));

beforeEach(() => {
  app.getVersion.mockReset();
});

describe("getRunningAppVersion", () => {
  it("reads the installed Tauri bundle version without relying on a global", async () => {
    app.getVersion.mockResolvedValue("0.1.1");

    await expect(getRunningAppVersion()).resolves.toBe("0.1.1");
  });

  it("uses the neutral placeholder path when browser preview has no Tauri API", async () => {
    app.getVersion.mockRejectedValue(new Error("Tauri unavailable"));

    await expect(getRunningAppVersion()).resolves.toBeNull();
  });
});
