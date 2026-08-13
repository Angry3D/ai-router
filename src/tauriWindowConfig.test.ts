import defaultCapabilities from "../src-tauri/capabilities/default.json";
import productionConfig from "../src-tauri/tauri.conf.json";
import qaConfig from "../src-tauri/tauri.qa.conf.json";
import { describe, expect, it } from "vitest";

type WindowConfig = {
  height?: number;
  hiddenTitle?: boolean;
  label: string;
  minHeight?: number;
  minWidth?: number;
  shadow?: boolean;
  titleBarStyle?: string;
  transparent?: boolean;
  visibleOnAllWorkspaces?: boolean;
  width?: number;
};

type TauriConfig = {
  app: {
    windows: WindowConfig[];
  };
  bundle?: {
    icon?: string[];
  };
};

describe.each([
  ["production", productionConfig],
  ["QA", qaConfig],
] as const)("%s Tauri window configuration", (_variant, rawConfig) => {
  const windows = (rawConfig as TauriConfig).app.windows;

  it("uses only the card shadow for the transparent menu", () => {
    expect(windows.find((window) => window.label === "menu")).toMatchObject({
      transparent: true,
      shadow: false,
    });
  });

  it("keeps the platform-default shadow for settings", () => {
    expect(
      windows.find((window) => window.label === "settings")?.shadow,
    ).toBeUndefined();
  });

  it("makes only the transient menu visible across Spaces", () => {
    expect(windows.find((window) => window.label === "menu")).toMatchObject({
      visibleOnAllWorkspaces: true,
    });
    expect(
      windows.find((window) => window.label === "settings")
        ?.visibleOnAllWorkspaces,
    ).toBeUndefined();
  });

  it("matches the overlay settings frame and supported minimum", () => {
    expect(windows.find((window) => window.label === "settings")).toMatchObject(
      {
        width: 920,
        height: 640,
        minWidth: 760,
        minHeight: 560,
        titleBarStyle: "Overlay",
        hiddenTitle: true,
      },
    );
  });
});

describe("Tauri window capabilities", () => {
  it("allows Settings to read the installed bundle version", () => {
    expect(defaultCapabilities.windows).toContain("settings");
    expect(defaultCapabilities.permissions).toContain("core:app:allow-version");
  });

  it("allows the overlay Settings drag region to start native movement", () => {
    expect(defaultCapabilities.windows).toContain("settings");
    expect(defaultCapabilities.permissions).toContain(
      "core:window:allow-start-dragging",
    );
  });

  it("allows both windows to follow the shared runtime appearance", () => {
    expect(defaultCapabilities.windows).toEqual(expect.arrayContaining(["menu", "settings"]));
    expect(defaultCapabilities.permissions).toContain("core:window:allow-set-theme");
  });
});

describe("Tauri app icon configuration", () => {
  it("uses the production app icon for the production bundle", () => {
    expect((productionConfig as TauriConfig).bundle?.icon).toEqual([
      "icons/app-icon.icns",
    ]);
  });

  it("keeps QA as the same product with an isolated bundle icon", () => {
    expect(qaConfig.identifier).toBe("com.relax.airouter.qa");
    expect((qaConfig as TauriConfig).bundle?.icon).toEqual([
      "icons/app-icon-qa.icns",
    ]);
  });
});
