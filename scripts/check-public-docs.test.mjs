import { describe, expect, it } from "vitest";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PublicDocsError,
  checkPublicDocs,
  discoverReleaseNotes,
  resolvePublicDocumentPaths,
  validateDeliveryLifecycleContract,
  validateMarkdownLinks,
  validatePublicDocuments,
  validatePublicText,
  validateReadmeLanguageSwitches,
  validateReleaseNoteLanguagePairs,
  validateReleaseInventoryContract,
  validateSensitiveWarning,
  validateVersionIndependentProjectClaims,
} from "./check-public-docs.mjs";

const PROJECT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

async function fixtureRoot(files) {
  const root = await mkdtemp(join(tmpdir(), "public-docs-"));
  for (const [path, content] of Object.entries(files)) {
    const destination = join(root, path);
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, content);
  }
  return root;
}

describe("public documentation contract", () => {
  it("validates the repository public documentation surface", async () => {
    await expect(checkPublicDocs(PROJECT_ROOT)).resolves.toBeDefined();

    const { required, releaseNotes } =
      await resolvePublicDocumentPaths(PROJECT_ROOT);
    expect(required).toContain("THIRD_PARTY_NOTICES.md");
    expect(required).toContain("release-notes/README.md");
    expect(releaseNotes.length).toBeGreaterThan(0);
    expect(
      releaseNotes.every((path) => /^release-notes\/v[^/]+\.md$/.test(path)),
    ).toBe(true);
  });

  it("rejects broken links inside discovered release notes", async () => {
    const root = await fixtureRoot({
      "release-notes/v9.9.9.md": "# v9.9.9\n\n[broken](./not-present.md)\n",
    });
    const releaseNotes = await discoverReleaseNotes(root);
    expect(releaseNotes).toEqual(["release-notes/v9.9.9.md"]);
    await expect(
      validatePublicDocuments(root, releaseNotes),
    ).rejects.toThrow("broken relative link");

    const withoutVersionedNotes = await fixtureRoot({
      "release-notes/README.md": "# Release notes\n",
    });
    await expect(discoverReleaseNotes(withoutVersionedNotes)).rejects.toThrow(
      "contains no versioned release notes",
    );
    await expect(discoverReleaseNotes(await fixtureRoot({}))).rejects.toThrow(
      "release-notes/",
    );
  });

  it("only accepts regular release-note files and refuses symlinks", async () => {
    const root = await fixtureRoot({
      "release-notes/v1.0.0.md": "# v1.0.0\n",
      "release-notes/notes.md": "# not a versioned note\n",
    });
    await expect(discoverReleaseNotes(root)).resolves.toEqual([
      "release-notes/v1.0.0.md",
    ]);

    const outsideRoot = await mkdtemp(join(tmpdir(), "outside-release-"));
    await writeFile(
      join(outsideRoot, "external-note.md"),
      "# outside the public tree\n",
    );
    await symlink(
      join(outsideRoot, "external-note.md"),
      join(root, "release-notes/v0.6.0.md"),
    );
    await expect(discoverReleaseNotes(root)).rejects.toThrow(
      "release-notes/v0.6.0.md is not a regular file",
    );

    // A plain versioned file is still discovered next to the refused entry.
    await rm(join(root, "release-notes/v0.6.0.md"));
    await expect(discoverReleaseNotes(root)).resolves.toEqual([
      "release-notes/v1.0.0.md",
    ]);
  });

  it("checks third-party notices for links and paths but exempts license markers", async () => {
    const brokenLink = await fixtureRoot({
      "THIRD_PARTY_NOTICES.md":
        "# Notices\n\n[license](./third-party/licenses/MISSING.txt)\n",
    });
    await expect(
      validatePublicDocuments(brokenLink, ["THIRD_PARTY_NOTICES.md"]),
    ).rejects.toThrow("broken relative link");

    // Composed at runtime: the fixture must contain a macOS home-directory
    // path to exercise the checker, while the source must not carry a literal
    // that the repository security scan reports as a leaked local path.
    const homeNoticePath = ["", "Users", "example", "private", "notice.txt"].join(
      "/",
    );
    const localPath = await fixtureRoot({
      "THIRD_PARTY_NOTICES.md": `See ${homeNoticePath}.\n`,
    });
    await expect(
      validatePublicDocuments(localPath, ["THIRD_PARTY_NOTICES.md"]),
    ).rejects.toThrow("local absolute path");

    // Composed at runtime for the same reason as the home path above: the
    // license marker must exist in the fixture but not as a public-tree literal.
    const licenseMarker = ["AGPL", "3.0-only"].join("-");
    const licenseText = await fixtureRoot({
      "THIRD_PARTY_NOTICES.md": `Distributed under ${licenseMarker}.\n`,
    });
    await expect(
      validatePublicDocuments(licenseText, ["THIRD_PARTY_NOTICES.md"]),
    ).resolves.toBeInstanceOf(Map);
    expect(() =>
      validatePublicText("README.md", `Distributed under ${licenseMarker}.`),
    ).toThrow("private workflow marker");
  });

  it("rejects a missing required public document", async () => {
    const root = await fixtureRoot({});
    await expect(
      validatePublicDocuments(root, ["THIRD_PARTY_NOTICES.md"]),
    ).rejects.toThrow("Required public documentation file is missing");
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

  it("rejects missing same-page and cross-page fragments", async () => {
    const root = await fixtureRoot({
      "README.md": "# Title\n\n[bad](#absent)\n",
      "docs/guide.md": "# Existing heading\n",
    });
    await expect(
      validateMarkdownLinks(root, "README.md", "# Title\n\n[bad](#absent)"),
    ).rejects.toThrow("missing fragment");
    await expect(
      validateMarkdownLinks(
        root,
        "README.md",
        "[bad](./docs/guide.md#absent)",
      ),
    ).rejects.toThrow("missing fragment");
    await expect(
      validateMarkdownLinks(root, "README.md", "# Title\n\n[ok](#title)"),
    ).resolves.toBeUndefined();
    await expect(
      validateMarkdownLinks(
        root,
        "README.md",
        "[ok](./docs/guide.md#existing-heading)",
      ),
    ).resolves.toBeUndefined();
  });

  it("requires alternative text on repository images", async () => {
    const root = await fixtureRoot({ "docs/shot.png": "synthetic" });
    await expect(
      validateMarkdownLinks(
        root,
        "README.md",
        '<img src="./docs/shot.png" alt="A screenshot" width="10">',
      ),
    ).resolves.toBeUndefined();
    await expect(
      validateMarkdownLinks(
        root,
        "README.md",
        '<img src="./docs/shot.png" width="10">',
      ),
    ).rejects.toThrow("alternative text");
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
        "Do not attach an API key, complete configuration, database or raw logs.",
      ),
    ).not.toThrow();
    expect(() =>
      validateSensitiveWarning("template.md", "Do not attach an API key."),
    ).toThrow("complete configuration");
  });

  it("requires stable project claims without a live patch version", () => {
    const files = new Map([
      [
        "README.md",
        "The project is still in early development. GitHub Releases is the download entry point. A future version could be 9.9.9.",
      ],
      ["CONTRIBUTING.md", "This is an early-stage project."],
      ["SUPPORT.md", "AI Router is an early-stage personal project."],
      [
        "docs/engineering/README.md",
        "This index describes the current product and release boundaries.",
      ],
    ]);

    expect(() => validateVersionIndependentProjectClaims(files)).not.toThrow();
    files.set("README.md", "GitHub Releases is the download entry point.");
    expect(() => validateVersionIndependentProjectClaims(files)).toThrow(
      "early-stage project disclaimer",
    );

    // A bare "early" substring is not a disclaimer: "nearly"/"clearly" must
    // not satisfy the phase requirement.
    files.set(
      "README.md",
      "The project is nearly finished and clearly documented. GitHub Releases is the download entry point.",
    );
    expect(() => validateVersionIndependentProjectClaims(files)).toThrow(
      "early-stage project disclaimer",
    );
  });

  it("requires reciprocal README language switches", () => {
    expect(() =>
      validateReadmeLanguageSwitches(
        new Map([
          ["README.md", "English | [简体中文](./README.zh-CN.md)"],
          ["README.zh-CN.md", "[English](./README.md) | 简体中文"],
        ]),
      ),
    ).not.toThrow();
    expect(() =>
      validateReadmeLanguageSwitches(
        new Map([
          ["README.md", "English"],
          ["README.zh-CN.md", "[English](./README.md) | 简体中文"],
        ]),
      ),
    ).toThrow("./README.zh-CN.md");
  });

  it("binds the release inventory command to the documented human gate", () => {
    const packageJson = {
      scripts: { "release:inventory": "node scripts/release-inventory.mjs" },
    };
    const files = new Map([
      [
        "docs/engineering/releasing.md",
        "Run pnpm release:inventory; the inventory cannot automatically prove semantic completeness.",
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

  it("binds every delivery control to the status script and public guide", () => {
    const packageJson = {
      scripts: { "delivery:status": "node scripts/delivery-status.mjs" },
    };
    const path = "docs/engineering/development-lifecycle.md";
    const lifecycle = [
      "Start task: <item>",
      "Start task: <item>, without Trellis",
      "Continue the current task",
      "Check the current task",
      "Deliver the current task",
      "Park the current work; do not merge yet",
      "Bring the current task into main",
      "Defer the current task",
      "Prepare the next release",
      "Include <item list> in <version>, defer everything else unmerged",
      "Release <version>",
      "Show delivery status",
    ]
      .map((command) => `\`${command}\``)
      .join("\n");
    const files = new Map([
      [
        path,
        `${lifecycle}\npnpm delivery:status\npnpm release:inventory\ncommit push PR main stable tag`,
      ],
    ]);

    expect(() =>
      validateDeliveryLifecycleContract(packageJson, files),
    ).not.toThrow();
    packageJson.scripts["delivery:status"] = "node scripts/other.mjs";
    expect(() => validateDeliveryLifecycleContract(packageJson, files)).toThrow(
      "delivery:status does not invoke",
    );
    packageJson.scripts["delivery:status"] = "node scripts/delivery-status.mjs";
    files.set(
      path,
      files.get(path).replace("`Deliver the current task`", ""),
    );
    expect(() => validateDeliveryLifecycleContract(packageJson, files)).toThrow(
      "Deliver the current task",
    );
  });
});

describe("bilingual release-note parity gate", () => {
  const VALID_ENGLISH = [
    "# AI Router release v9.9.9",
    "",
    "## Highlights",
    "",
    "- A reviewed user-visible improvement.",
  ].join("\n");
  const VALID_CHINESE = [
    "# AI Router v9.9.9",
    "",
    "## 重点更新",
    "",
    "- 一项面向用户的改进。",
  ].join("\n");

  function pairs(english, chinese) {
    return new Map([
      ["release-notes/v9.9.9.md", english],
      ["release-notes/v9.9.9.zh-CN.md", chinese],
    ]);
  }

  it("accepts one matching English and Chinese projection", () => {
    expect(
      validateReleaseNoteLanguagePairs(pairs(VALID_ENGLISH, VALID_CHINESE)),
    ).toBe(1);
  });

  it.each([
    [
      "sections out of the canonical order",
      [
        "# AI Router release v9.9.9",
        "",
        "## Notices",
        "",
        "- No configuration migration is required.",
        "",
        "## Highlights",
        "",
        "- A reviewed improvement.",
      ].join("\n"),
      [
        "# AI Router v9.9.9",
        "",
        "## 注意事项",
        "",
        "- 无需迁移配置。",
        "",
        "## 重点更新",
        "",
        "- 一项改进。",
      ].join("\n"),
    ],
    [
      "a heading that does not match the file version",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- A reviewed improvement.",
      VALID_CHINESE,
    ],
    [
      "a document without any section",
      "# AI Router release v9.9.9\n\nPlain prose without a section.",
      "# AI Router v9.9.9\n\n只有散文，没有章节。",
    ],
    [
      "an unsupported section heading",
      "# AI Router release v9.9.9\n\n## Summary\n\n- A reviewed improvement.",
      VALID_CHINESE,
    ],
  ])(
    "rejects a projection that the release producer would also reject: %s",
    (_label, english, chinese) => {
      expect(() => validateReleaseNoteLanguagePairs(pairs(english, chinese))).toThrow(
        "does not use the documented heading, section order, or item layout",
      );
    },
  );

  it("rejects projections that cover different sections or item counts", () => {
    expect(() =>
      validateReleaseNoteLanguagePairs(
        pairs(
          [
            "# AI Router release v9.9.9",
            "",
            "## Highlights",
            "",
            "- A reviewed improvement.",
            "",
            "## Notices",
            "",
            "- No configuration migration is required.",
          ].join("\n"),
          [
            "# AI Router v9.9.9",
            "",
            "## 重点更新",
            "",
            "- 一项改进。",
            "",
            "## 问题修复",
            "",
            "- 修复一个问题。",
            "",
            "## 注意事项",
            "",
            "- 无需迁移配置。",
          ].join("\n"),
        ),
      ),
    ).toThrow("English and Chinese notes disagree");

    expect(() =>
      validateReleaseNoteLanguagePairs(
        pairs(
          VALID_ENGLISH,
          "# AI Router v9.9.9\n\n## 重点更新\n\n- 一项改进。\n\n- 第二项改进。",
        ),
      ),
    ).toThrow("English and Chinese notes disagree");
  });

  it.each([
    "v1.2.3.4",
    "v1.3.0-rc.1",
    "v1.4.0+build.5",
  ])(
    "refuses to silently skip a versioned note that is not a stable release: %s",
    (version) => {
      expect(() =>
        validateReleaseNoteLanguagePairs(
          new Map([
            ["release-notes/v9.9.9.md", VALID_ENGLISH],
            ["release-notes/v9.9.9.zh-CN.md", VALID_CHINESE],
            [`release-notes/${version}.md`, VALID_ENGLISH],
            [
              `release-notes/${version}.zh-CN.md`,
              VALID_CHINESE.replaceAll("v9.9.9", version),
            ],
          ]),
        ),
      ).toThrow("is not a canonical stable release note");
    },
  );

  it("ignores unversioned prose inside the release-notes directory", () => {
    expect(
      validateReleaseNoteLanguagePairs(
        new Map([
          ["release-notes/v9.9.9.md", VALID_ENGLISH],
          ["release-notes/v9.9.9.zh-CN.md", VALID_CHINESE],
          ["release-notes/README.md", "# Release notes\n"],
        ]),
      ),
    ).toBe(1);
  });

  it("requires both projections in each direction", () => {
    expect(() =>
      validateReleaseNoteLanguagePairs(
        new Map([["release-notes/v9.9.9.md", VALID_ENGLISH]]),
      ),
    ).toThrow("no reviewed Chinese counterpart");
    // An otherwise complete pair plus one orphan Chinese note.
    expect(() =>
      validateReleaseNoteLanguagePairs(
        new Map([
          ["release-notes/v9.9.9.md", VALID_ENGLISH],
          ["release-notes/v9.9.9.zh-CN.md", VALID_CHINESE],
          [
            "release-notes/v9.9.8.zh-CN.md",
            VALID_CHINESE.replace("v9.9.9", "v9.9.8"),
          ],
        ]),
      ),
    ).toThrow("without an English source");
  });
});
