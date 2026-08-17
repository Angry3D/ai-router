import { describe, expect, it } from "vitest";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PublicDocsError,
  checkPublicDocs,
  validateMarkdownLinks,
  validatePublicText,
  validateSensitiveWarning,
  validateVersionIndependentProjectClaims,
} from "./check-public-docs.mjs";

const PROJECT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

describe("public documentation contract", () => {
  it("accepts the repository public documentation surface", async () => {
    await expect(checkPublicDocs(PROJECT_ROOT)).resolves.toMatchObject({
      requiredFiles: 19,
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
});
