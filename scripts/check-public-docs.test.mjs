import { describe, expect, it } from "vitest";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PublicDocsError,
  checkPublicDocs,
  validateMarkdownLinks,
  validatePublicText,
  validateReleaseInventoryContract,
  validateSensitiveWarning,
  validateVersionIndependentProjectClaims,
} from "./check-public-docs.mjs";

const PROJECT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

describe("public documentation contract", () => {
  it("accepts the repository public documentation surface", async () => {
    await expect(checkPublicDocs(PROJECT_ROOT)).resolves.toMatchObject({
      requiredFiles: 20,
    });
  });

  it("rejects broken relative links", async () => {
    await expect(
      validateMarkdownLinks(
        PROJECT_ROOT,
        "README.md",
        "[missing](./not-present.md)",
      ),
    ).rejects.toThrow("broken relative link");
  });

  it("rejects private markers and local absolute paths", () => {
    const privateMarker = `<!-- ${"TRELLIS"}:START -->`;
    expect(() => validatePublicText("README.md", privateMarker)).toThrow(
      PublicDocsError,
    );

    const localPath = ["", "Users", "example", "private", "note.txt"].join("/");
    expect(() => validatePublicText("README.md", localPath)).toThrow(
      "local absolute path",
    );
  });

  it("requires every sensitive-data warning category", () => {
    expect(() =>
      validateSensitiveWarning(
        "template.md",
        "不要提交 API Key、完整配置、数据库或原始日志。",
      ),
    ).not.toThrow();
    expect(() =>
      validateSensitiveWarning("template.md", "不要提交 API Key。"),
    ).toThrow("完整配置");
  });

  it("requires stable project claims without a live patch version", () => {
    const files = new Map([
      [
        "README.md",
        "项目仍处于早期开发阶段。下载入口为 GitHub Releases。未来版本可以是 9.9.9。",
      ],
      ["CONTRIBUTING.md", "项目仍处于早期阶段。"],
      ["SUPPORT.md", "AI Router 是早期个人维护项目。"],
      ["docs/engineering/README.md", "本文描述当前产品和发布边界。"],
    ]);

    expect(() => validateVersionIndependentProjectClaims(files)).not.toThrow();
    files.set("README.md", "下载入口为 GitHub Releases。");
    expect(() => validateVersionIndependentProjectClaims(files)).toThrow(
      "项目仍处于早期开发阶段",
    );
  });

  it("binds the release inventory command to the documented human gate", () => {
    const packageJson = {
      scripts: { "release:inventory": "node scripts/release-inventory.mjs" },
    };
    const files = new Map([
      [
        "docs/engineering/releasing.md",
        "运行 pnpm release:inventory；不能自动证明语义完整性。",
      ],
      [
        "release-notes/README.md",
        "Run pnpm release:inventory and record a concise exclusion reason.",
      ],
    ]);

    expect(() =>
      validateReleaseInventoryContract(packageJson, files),
    ).not.toThrow();
    packageJson.scripts["release:inventory"] = "node scripts/other.mjs";
    expect(() => validateReleaseInventoryContract(packageJson, files)).toThrow(
      "release:inventory does not invoke",
    );
    packageJson.scripts["release:inventory"] =
      "node scripts/release-inventory.mjs";
    files.set("release-notes/README.md", "pnpm release:inventory");
    expect(() => validateReleaseInventoryContract(packageJson, files)).toThrow(
      "record a concise exclusion reason",
    );
  });
});
