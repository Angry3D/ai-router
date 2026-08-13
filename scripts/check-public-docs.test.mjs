import { describe, expect, it } from "vitest";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PublicDocsError,
  checkPublicDocs,
  validateMarkdownLinks,
  validatePublicText,
  validateSensitiveWarning,
} from "./check-public-docs.mjs";

const PROJECT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

describe("public documentation contract", () => {
  it("accepts the repository public documentation surface", async () => {
    await expect(checkPublicDocs(PROJECT_ROOT)).resolves.toMatchObject({
      requiredFiles: 17,
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
});
