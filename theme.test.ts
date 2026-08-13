import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const readProjectFile = (relativePath: string) =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

describe("shared appearance theme contract", () => {
  it("allows both WebView documents to render light or dark controls", () => {
    for (const html of [readProjectFile("./menu.html"), readProjectFile("./settings.html")]) {
      expect(html).toContain('<meta name="color-scheme" content="light dark" />');
    }
  });

  it("uses one semantic data-theme dark branch", () => {
    const styles = readProjectFile("./src/styles.css");

    expect(styles).toContain("color-scheme: light;");
    expect(styles).toContain(':root[data-theme="dark"]');
    expect(styles).not.toContain("filter: invert");
    expect(styles).not.toContain("data-preview-theme");
  });

  it("keeps state colors semantic and the approved appearance geometry", () => {
    const styles = readProjectFile("./src/styles.css");

    expect(styles).toContain("--variant-badge-surface:");
    expect(styles).toContain("--warning-control-surface:");
    expect(styles).toContain("--success-boundary-line:");
    expect(styles).toContain("--switch-track:");
    expect(styles).toContain("--usage-output:");
    expect(styles).toContain("--latency-warning:");
    expect(styles).toMatch(
      /\.settings-segments-three\s*\{[^}]*max-width:\s*300px;/,
    );
  });

  it("lets the appearance provider control both native windows", () => {
    const config = JSON.parse(readProjectFile("./src-tauri/tauri.conf.json")) as {
      app: { windows: Array<{ label: string; theme?: string }> };
    };

    expect(config.app.windows.map((window) => window.label)).toEqual(["menu", "settings"]);
    expect(config.app.windows.every((window) => window.theme === undefined)).toBe(true);
  });
});
