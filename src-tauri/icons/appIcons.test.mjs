import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const iconDirectory = dirname(fileURLToPath(import.meta.url));
const routePath = 'd="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"';

function pngMetadata(buffer) {
  expect(buffer.subarray(0, 8)).toEqual(
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  );
  expect(buffer.subarray(12, 16).toString("ascii")).toBe("IHDR");
  return {
    colorType: buffer[25],
    height: buffer.readUInt32BE(20),
    width: buffer.readUInt32BE(16),
  };
}

describe("macOS app icon assets", () => {
  it("uses the exact Route geometry for production and QA sources", async () => {
    const [production, qa, tray] = await Promise.all([
      readFile(join(iconDirectory, "app-icon.svg"), "utf8"),
      readFile(join(iconDirectory, "app-icon-qa.svg"), "utf8"),
      readFile(join(iconDirectory, "tray-route.svg"), "utf8"),
    ]);

    for (const source of [production, qa]) {
      expect(source).toContain('width="1024" height="1024"');
      expect(source).toContain('<circle cx="6" cy="19" r="3"/>');
      expect(source).toContain(routePath);
      expect(source).toContain('<circle cx="18" cy="5" r="3"/>');
    }
    expect(tray).toContain(routePath);
    expect(production).not.toContain('x="670" y="705"');
    expect(qa).toContain('x="670" y="705"');
  });

  it("ships 512px RGBA previews and macOS icon containers for both variants", async () => {
    const [productionPng, qaPng, defaultPng, productionIcns, qaIcns] = await Promise.all([
      readFile(join(iconDirectory, "app-icon.png")),
      readFile(join(iconDirectory, "app-icon-qa.png")),
      readFile(join(iconDirectory, "icon.png")),
      readFile(join(iconDirectory, "app-icon.icns")),
      readFile(join(iconDirectory, "app-icon-qa.icns")),
    ]);

    for (const png of [productionPng, qaPng]) {
      expect(pngMetadata(png)).toEqual({
        colorType: 6,
        height: 512,
        width: 512,
      });
    }
    expect(productionIcns.subarray(0, 4).toString("ascii")).toBe("icns");
    expect(qaIcns.subarray(0, 4).toString("ascii")).toBe("icns");
    expect(defaultPng).toEqual(productionPng);
  });
});
