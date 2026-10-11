import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  assertReleaseNoteProjectionsMatch,
  createLatestManifest,
  expectedAssetNames,
  inspectAppBundle,
  loadReleaseNotes,
  parseChineseReleaseNotes,
  parseReleaseNotes,
  ReleaseError,
  releaseNotes,
  renderReleaseConfig,
  uploadPreparedDraft,
  validateDraftBody,
  validateReleaseIdentity,
  validateRemoteAssetInventory,
  validateRepairableDraft,
  validateUpdaterPublicKey,
  verifyReleaseDirectory,
} from "./manage-release.mjs";

const temporaryRoots = [];

function updaterPublicKey() {
  const rawKey = Buffer.alloc(42, 7).toString("base64");
  return Buffer.from(
    `untrusted comment: minisign public key test fixture\n${rawKey}\n`,
  ).toString("base64");
}

function digest(content) {
  return createHash("sha256").update(content).digest("hex");
}

function releaseDocument(version = "1.2.3") {
  return assertReleaseNoteProjectionsMatch(
    parseReleaseNotes(
      `# AI Router release v${version}\n\n## Highlights\n\n- Reviewed release notes are available for users.`,
      version,
    ),
    parseChineseReleaseNotes(
      `# AI Router v${version}\n\n## 重点更新\n\n- 本版本为重点更新内容。`,
      version,
    ),
    version,
  );
}

async function releaseFixture(version = "1.2.3") {
  const root = await mkdtemp(join(tmpdir(), "ai-router-release-"));
  temporaryRoots.push(root);
  await mkdir(root, { recursive: true });
  const names = expectedAssetNames(version);
  const signature = Buffer.from("synthetic minisign signature").toString(
    "base64",
  );
  const content = new Map([
    [names[0], Buffer.from("synthetic dmg")],
    [names[1], Buffer.from("synthetic updater archive")],
    [names[2], Buffer.from(`${signature}\n`)],
  ]);
  for (const [name, bytes] of content) {
    await writeFile(join(root, name), bytes);
  }
  const document = releaseDocument(version);
  const manifest = createLatestManifest(
    version,
    "2026-08-16T10:00:00.000Z",
    signature,
    document,
  );
  const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
  content.set("latest.json", manifestBytes);
  await writeFile(join(root, "latest.json"), manifestBytes);
  const checksums = names
    .slice(0, 4)
    .map((name) => `${digest(content.get(name))}  ${name}`)
    .join("\n");
  await writeFile(join(root, "SHA256SUMS"), `${checksums}\n`);
  return { document, names, root, version };
}

afterEach(async () => {
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { force: true, recursive: true })),
  );
});

describe("release identity", () => {
  it("binds one canonical stable version to its GitHub tag and commit", () => {
    const sha = "a".repeat(40);
    expect(
      validateReleaseIdentity(
        {
          repository: "Angry3D/ai-router",
          ref: "refs/tags/v1.2.3",
          refName: "v1.2.3",
          refType: "tag",
          sha,
        },
        "1.2.3",
      ),
    ).toEqual({
      repository: "Angry3D/ai-router",
      sha,
      tag: "v1.2.3",
      version: "1.2.3",
    });
  });

  it.each([
    ["refs/tags/v1.2.4", "v1.2.4", "1.2.3"],
    ["refs/heads/main", "main", "1.2.3"],
    ["refs/tags/v1.2.3-beta.1", "v1.2.3-beta.1", "1.2.3-beta.1"],
    ["refs/tags/v1.2.3+build.1", "v1.2.3+build.1", "1.2.3+build.1"],
  ])(
    "rejects a drifted or unstable release ref %s",
    (ref, refName, version) => {
      expect(() =>
        validateReleaseIdentity(
          {
            repository: "Angry3D/ai-router",
            ref,
            refName,
            refType: "tag",
            sha: "a".repeat(40),
          },
          version,
        ),
      ).toThrow(ReleaseError);
    },
  );

  it("permits repair only for an unpublished draft bound to the same commit", () => {
    const identity = {
      repository: "Angry3D/ai-router",
      sha: "a".repeat(40),
      tag: "v1.2.3",
      version: "1.2.3",
    };
    expect(
      validateRepairableDraft(
        {
          assets: [],
          isDraft: true,
          tagName: identity.tag,
          targetCommitish: identity.sha,
        },
        identity,
      ),
    ).toMatchObject({ isDraft: true });
    expect(() =>
      validateRepairableDraft(
        {
          assets: [],
          isDraft: false,
          tagName: identity.tag,
          targetCommitish: identity.sha,
        },
        identity,
      ),
    ).toThrow("repairable draft boundary");
    expect(() =>
      validateRepairableDraft(
        {
          assets: [],
          isDraft: true,
          tagName: identity.tag,
          targetCommitish: "b".repeat(40),
        },
        identity,
      ),
    ).toThrow("repairable draft boundary");
  });
});

describe("release draft upload", () => {
  const identity = {
    repository: "Angry3D/ai-router",
    sha: "a".repeat(40),
    tag: "v1.2.3",
    version: "1.2.3",
  };

  function runnerForRelease(release, calls) {
    return async (command, args) => {
      calls.push({ args, command });
      if (command === "gh" && args[0] === "release" && args[1] === "view") {
        return {
          code: 0,
          stderr: "",
          stdout: JSON.stringify(release),
        };
      }
      if (command === "gh" && args[0] === "release" && args[1] === "upload") {
        return { code: 0, stderr: "", stdout: "" };
      }
      throw new Error(`Unexpected command in release fixture: ${command}`);
    };
  }

  it.each([
    {
      assets: [],
      isDraft: false,
      tagName: identity.tag,
      targetCommitish: identity.sha,
    },
    {
      assets: [{ name: "existing.dmg", size: 1 }],
      isDraft: true,
      tagName: identity.tag,
      targetCommitish: identity.sha,
    },
  ])(
    "refuses a published or non-empty release before upload",
    async (release) => {
      const calls = [];
      await expect(
        uploadPreparedDraft(
          identity,
          "synthetic-release-directory",
          runnerForRelease(release, calls),
        ),
      ).rejects.toThrow(ReleaseError);
      expect(
        calls.some(
          ({ args, command }) =>
            command === "gh" && args[0] === "release" && args[1] === "upload",
        ),
      ).toBe(false);
    },
  );

  it("uploads only to a freshly revalidated empty draft without clobbering", async () => {
    const calls = [];
    await uploadPreparedDraft(
      identity,
      "synthetic-release-directory",
      runnerForRelease(
        {
          assets: [],
          isDraft: true,
          tagName: identity.tag,
          targetCommitish: identity.sha,
        },
        calls,
      ),
    );
    const upload = calls.find(
      ({ args, command }) =>
        command === "gh" && args[0] === "release" && args[1] === "upload",
    );
    expect(upload).toBeDefined();
    expect(upload.args).not.toContain("--clobber");
    expect(upload.args.slice(-2)).toEqual(["--repo", identity.repository]);
    expect(calls.map(({ args }) => args[1])).toEqual(["view", "upload"]);
  });
});

describe("remote draft inventory", () => {
  const version = "1.2.3";
  const names = expectedAssetNames(version);
  const validAssets = [names[1], names[2], names[0], names[3], names[4]].map(
    (name, index) => ({ name, size: index + 1 }),
  );

  it("accepts the complete GitHub asset order without locale-dependent sorting", () => {
    expect(() =>
      validateRemoteAssetInventory(validAssets, version),
    ).not.toThrow();
  });

  it.each([
    validAssets.slice(1),
    [...validAssets.slice(0, -1), { name: "unexpected.txt", size: 5 }],
    [validAssets[0], validAssets[0], ...validAssets.slice(2)],
    [{ ...validAssets[0], size: 0 }, ...validAssets.slice(1)],
    [{ ...validAssets[0], size: 1.5 }, ...validAssets.slice(1)],
  ])(
    "rejects an incomplete, unexpected, duplicate, or invalid inventory",
    (assets) => {
      expect(() => validateRemoteAssetInventory(assets, version)).toThrow(
        "inventory",
      );
    },
  );
});

describe("release signing configuration", () => {
  it("accepts a bounded base64-encoded minisign public key and injects it once", () => {
    const publicKey = updaterPublicKey();
    expect(validateUpdaterPublicKey(publicKey)).toBe(publicKey);
    const template = JSON.stringify({
      bundle: {
        targets: ["app", "dmg"],
        createUpdaterArtifacts: true,
        resources: {
          "../LICENSE": "LICENSE",
          "../THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md",
        },
        macOS: { signingIdentity: "-" },
      },
      plugins: {
        updater: { pubkey: "__AI_ROUTER_UPDATER_PUBLIC_KEY__" },
      },
    });
    const rendered = JSON.parse(renderReleaseConfig(template, publicKey));
    expect(rendered.plugins.updater.pubkey).toBe(publicKey);
    expect(JSON.stringify(rendered)).not.toContain(
      "__AI_ROUTER_UPDATER_PUBLIC_KEY__",
    );
  });

  it("rejects a DMG-only release configuration before build", () => {
    const template = JSON.stringify({
      bundle: {
        targets: ["dmg"],
        createUpdaterArtifacts: true,
        resources: {
          "../LICENSE": "LICENSE",
          "../THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md",
        },
        macOS: { signingIdentity: "-" },
      },
      plugins: {
        updater: { pubkey: "__AI_ROUTER_UPDATER_PUBLIC_KEY__" },
      },
    });

    expect(() => renderReleaseConfig(template, updaterPublicKey())).toThrow(
      "distribution contract",
    );
  });

  it.each([
    "",
    "__AI_ROUTER_UPDATER_PUBLIC_KEY__",
    Buffer.from("synthetic PRIVATE KEY marker").toString("base64"),
    Buffer.from("not a minisign public key").toString("base64"),
  ])("rejects an absent, private, or malformed updater public key", (value) => {
    expect(() => validateUpdaterPublicKey(value)).toThrow(ReleaseError);
  });
});

async function rewriteManifest(fixture, mutate) {
  const manifestPath = join(fixture.root, "latest.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  mutate(manifest);
  const bytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(manifestPath, bytes);
  // `latest.json` is covered by SHA256SUMS, so a mutated manifest must be
  // re-checksummed. Otherwise the checksum mismatch would reject the fixture
  // first and the test would pass without exercising the projection checks.
  const checksums = [];
  for (const name of fixture.names.slice(0, 4)) {
    const bytes = name === "latest.json" ? readFileSync(join(fixture.root, name)) : await readFile(join(fixture.root, name));
    checksums.push(`${digest(bytes)}  ${name}`);
  }
  await writeFile(join(fixture.root, "SHA256SUMS"), `${checksums.join("\n")}\n`);
}

describe("release metadata and inventory", () => {
  it("produces user-facing DMG guidance without claiming Apple verification", () => {
    const notes = releaseNotes("1.2.3", releaseDocument());
    expect(notes).toContain("For a first-time install, download the DMG");
    expect(notes).toContain(
      "not verified or notarized with an Apple Developer ID",
    );
  });

  it("does not generate either release output without reviewed notes", () => {
    const signature = Buffer.from("synthetic minisign signature").toString(
      "base64",
    );
    expect(() => releaseNotes("1.2.3")).toThrow(
      "both reviewed language projections",
    );
    expect(() =>
      createLatestManifest("1.2.3", "2026-08-16T10:00:00.000Z", signature),
    ).toThrow("both reviewed language projections");
  });

  it("parses one reviewed document for both release and updater output", () => {
    const document = assertReleaseNoteProjectionsMatch(
      parseReleaseNotes(
      [
        "# AI Router release v1.2.3",
        "",
        "## Highlights",
        "",
        "- Adds a user-visible update summary.",
        "- Streamlines the update flow in the settings window.",
        "",
        "## Fixes",
        "",
        "- Fixes one user-visible problem.",
        "",
        "## Notices",
        "",
        "- This version requires no configuration migration.",
      ].join("\n"),
      "1.2.3",
    ),
      parseChineseReleaseNotes(
        [
          "# AI Router v1.2.3",
          "",
          "## 重点更新",
          "",
          "- 新增面向用户的更新摘要。",
          "- 简化设置窗口中的更新流程。",
          "",
          "## 问题修复",
          "",
          "- 修复一个面向用户的问题。",
          "",
          "## 注意事项",
          "",
          "- 本版本无需迁移配置。",
        ].join("\n"),
        "1.2.3",
      ),
      "1.2.3",
    );
    const manifest = createLatestManifest(
      "1.2.3",
      "2026-08-16T10:00:00.000Z",
      Buffer.from("synthetic minisign signature").toString("base64"),
      document,
    );

    // `notes` must stay the Chinese projection an installed reader already
    // parses, and the sibling locale map is additive.
    expect(manifest.notes).toBe(document.chinese.markdown);
    expect(manifest.notes_i18n["zh-CN"]).toBe(document.chinese.markdown);
    expect(manifest.notes_i18n.en).toBe(document.english.markdown);
    const body = releaseNotes("1.2.3", document);
    expect(body).toContain(document.chinese.markdown);
    expect(body).toContain(document.english.markdown);
    expect(body.indexOf(document.chinese.markdown)).toBeLessThan(
      body.indexOf(document.english.markdown),
    );
    expect(body).toContain("中文 | English");
    expect(document.english.highlights).toHaveLength(2);
    expect(document.english.fixes).toHaveLength(1);
    expect(document.english.notices).toHaveLength(1);
  });

  it("refuses to publish from a single-language release document", () => {
    const englishOnly = parseReleaseNotes(
      "# AI Router release v1.2.3\n\n## Highlights\n\n- Reviewed release notes are available for users.",
      "1.2.3",
    );
    expect(() => releaseNotes("1.2.3", englishOnly)).toThrow(
      "both reviewed language projections",
    );
  });

  // The generic-content guard runs on parsed items that may be Chinese, so its
  // Chinese alternatives must survive. Without them a Chinese installer
  // boilerplate note would pass the gate and ship as a real release body.
  it("rejects Chinese installer boilerplate as generic placeholder content", () => {
    expect(() =>
      parseChineseReleaseNotes(
        [
          "# AI Router v1.2.3",
          "",
          "## 重点更新",
          "",
          "- 首次安装请下载 DMG，该应用由 AI Router v1.2.3 已发布。",
        ].join("\n"),
        "1.2.3",
      ),
    ).toThrow("generic placeholder content is not sufficient");

    expect(() =>
      parseReleaseNotes(
        "# AI Router release v1.2.3\n\n## Highlights\n\n- First-time installs download the DMG and the updater signature is applied.",
        "1.2.3",
      ),
    ).toThrow("generic placeholder content is not sufficient");

    // Real reviewed content must keep parsing.
    expect(() =>
      parseChineseReleaseNotes(
        "# AI Router v1.2.3\n\n## 重点更新\n\n- 设置窗口新增应用内更新入口，可直接查看版本说明。",
        "1.2.3",
      ),
    ).not.toThrow();
  });

  // The pair assertion must hold at the public renderers too, not only inside
  // `loadReleaseNotes`: both are exported and called directly.
  it("refuses to render or sign a manifest from a mismatched pair", () => {
    const mismatched = {
      english: parseReleaseNotes(
        "# AI Router release v1.2.3\n\n## Highlights\n\n- One item.\n\n## Notices\n\n- A notice.",
        "1.2.3",
      ),
      chinese: parseChineseReleaseNotes(
        "# AI Router v1.2.3\n\n## 重点更新\n\n- 只有一项内容。",
        "1.2.3",
      ),
    };
    expect(() =>
      createLatestManifest(
        "1.2.3",
        "2026-08-16T10:00:00.000Z",
        Buffer.from("synthetic minisign signature").toString("base64"),
        mismatched,
      ),
    ).toThrow("items but the English source has");
    expect(() => releaseNotes("1.2.3", mismatched)).toThrow(
      "items but the English source has",
    );
  });

  it("rejects a Chinese projection that describes a different release", () => {
    expect(() =>
      assertReleaseNoteProjectionsMatch(
        parseReleaseNotes(
          "# AI Router release v1.2.3\n\n## Highlights\n\n- One reviewed item.\n\n## Notices\n\n- A reviewed notice.",
          "1.2.3",
        ),
        parseChineseReleaseNotes(
          "# AI Router v1.2.3\n\n## 重点更新\n\n- 只有一项内容。",
          "1.2.3",
        ),
        "1.2.3",
      ),
    ).toThrow("items but the English source has");
  });

  it("rejects draft body drift before publication", () => {
    const document = releaseDocument();
    const body = releaseNotes("1.2.3", document);
    expect(() => validateDraftBody({ body }, body)).not.toThrow();
    expect(() => validateDraftBody({ body: `${body}\nDrift` }, body)).toThrow(
      "does not match",
    );
  });

  it.each([
    [
      "wrong version",
      "# AI Router release v9.9.9\n\n## Highlights\n\n- A valid improvement.",
    ],
    [
      "missing highlights",
      "# AI Router release v1.2.3\n\n## Fixes\n\n- A valid fix.",
    ],
    [
      "paragraph",
      "# AI Router release v1.2.3\n\n## Highlights\n\nA plain paragraph.",
    ],
    [
      "duplicate",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- Duplicate item.\n- Duplicate item.",
    ],
    [
      "link",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- [External link](https://example.invalid)",
    ],
    [
      "placeholder",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- AI Router release v1.2.3 released",
    ],
    [
      "too many highlights",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- One\n- Two\n- Three\n- Four",
    ],
    [
      "too many items",
      `# AI Router release v1.2.3\n\n## Highlights\n\n- Highlight\n\n## Fixes\n\n${Array.from({ length: 20 }, (_, index) => `- Fix ${index + 1}`).join("\n")}`,
    ],
    [
      "oversized item",
      `# AI Router release v1.2.3\n\n## Highlights\n\n- ${"x".repeat(241)}`,
    ],
    [
      "secret-like content",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- Configures TAURI_SIGNING_PRIVATE_KEY",
    ],
    [
      "control character",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- Contains\ta tab",
    ],
    [
      "inline markdown",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- **Bold placeholder**",
    ],
    [
      "TODO placeholder",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- TODO: add details",
    ],
    [
      "Chinese placeholder 待补充",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- 待补充",
    ],
    [
      "Chinese placeholder 待定 with punctuation",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- 待定。",
    ],
    [
      "Chinese placeholder 稍后补充",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- 路由回退改进已记录。\n\n## Fixes\n\n- 稍后补充",
    ],
    [
      "Chinese placeholder 待完善 with detail marker",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- 路由回退改进已记录。\n\n## Notices\n\n- 待完善：细节尚未确认",
    ],
    [
      "local path",
      `# AI Router release v1.2.3\n\n## Highlights\n\n- Debug file at ${[
        "",
        "Users",
        "example",
        "private.log",
      ].join("/")}`,
    ],
    [
      "empty optional section",
      "# AI Router release v1.2.3\n\n## Highlights\n\n- A valid improvement.\n\n## Fixes",
    ],
  ])("rejects invalid reviewed release notes: %s", (_label, markdown) => {
    expect(() => parseReleaseNotes(markdown, "1.2.3")).toThrow(ReleaseError);
  });

  it("loads only the exact versioned release-note file", async () => {
    const root = await mkdtemp(join(tmpdir(), "ai-router-release-notes-"));
    temporaryRoots.push(root);
    await mkdir(join(root, "release-notes"), { recursive: true });
    await writeFile(
      join(root, "release-notes", "v1.2.3.md"),
      "# AI Router release v1.2.3\n\n## Highlights\n\n- Reviewed release notes are available for users.\n",
    );
    await writeFile(
      join(root, "release-notes", "v1.2.3.zh-CN.md"),
      "# AI Router v1.2.3\n\n## 重点更新\n\n- 已审核的发布说明可供用户查看。\n",
    );
    await expect(loadReleaseNotes(root, "1.2.3")).resolves.toMatchObject({
      english: {
        highlights: ["Reviewed release notes are available for users."],
      },
      chinese: {
        highlights: ["已审核的发布说明可供用户查看。"],
      },
    });
    await writeFile(
      join(root, "release-notes", "v1.2.3.zh-CN.md"),
      "# AI Router v1.2.3\n\n## 重点更新\n\n- 第一项。\n\n- 第二项。\n",
    );
    await expect(loadReleaseNotes(root, "1.2.3")).rejects.toThrow(
      "Chinese projection of v1.2.3",
    );
    await writeFile(
      join(root, "release-notes", "v1.2.3.zh-CN.md"),
      "# AI Router v1.2.3\n\n## 重点更新\n\n- 已审核的发布说明可供用户查看。\n",
    );
    await rm(join(root, "release-notes", "v1.2.3.zh-CN.md"));
    await expect(loadReleaseNotes(root, "1.2.3")).rejects.toThrow(
      "missing reviewed Chinese file",
    );
    await expect(loadReleaseNotes(root, "1.2.4")).rejects.toThrow(
      "missing reviewed file",
    );
    await expect(loadReleaseNotes(root, "../../private")).rejects.toThrow(
      "canonical stable SemVer",
    );
  });

  it("accepts one complete internally consistent release directory", async () => {
    const fixture = await releaseFixture();
    await expect(
      verifyReleaseDirectory(fixture.root, fixture.version),
    ).resolves.toMatchObject({ assets: fixture.names });
  });

  it("counts Unicode code points and rejects manifest note drift", async () => {
    expect(() =>
      parseReleaseNotes(
        `# AI Router release v1.2.3\n\n## Highlights\n\n- ${"😀".repeat(240)}`,
        "1.2.3",
      ),
    ).not.toThrow();

    const fixture = await releaseFixture();
    const manifestPath = join(fixture.root, "latest.json");
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    // Drift the Chinese projection that `notes` carries, keeping the map
    // internally consistent so only the reviewed-source comparison can catch it.
    manifest.notes = manifest.notes.replace("本版本为重点更新内容。", "已被改写的内容。");
    manifest.notes_i18n["zh-CN"] = manifest.notes;
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    await expect(
      verifyReleaseDirectory(
        fixture.root,
        fixture.version,
        fixture.document,
      ),
    ).rejects.toThrow("reviewed source");
  });

  // `notes` is what every already-installed reader parses, so the manifest must
  // never be able to satisfy its own checks while drifting from that invariant.
  it.each([
    ["an empty locale projection", (m) => (m.notes_i18n.en = "")],
    [
      "an arbitrary string in a locale projection",
      (m) => (m.notes_i18n.en = "x"),
    ],
    [
      "a Chinese body mislabelled as English",
      (m) => (m.notes_i18n.en = m.notes),
    ],
    [
      "a non-canonical English rendering",
      (m) => (m.notes_i18n.en = `\n${m.notes_i18n.en}\n`),
    ],
    [
      "notes drifting away from the Chinese locale",
      (m) => (m.notes_i18n.en = m.notes),
    ],
  ])("rejects a manifest with %s", async (_label, mutate) => {
    const fixture = await releaseFixture();
    await rewriteManifest(fixture, mutate);
    await expect(
      verifyReleaseDirectory(
        fixture.root,
        fixture.version,
        fixture.document,
      ),
    ).rejects.toThrow(
      /notes_i18n|reviewed source|Release notes are invalid/,
    );
  });

  // The locale map must hold its invariants even when no reviewed document is
  // supplied for comparison, because verification is a public entry point that
  // callers may invoke with the reviewed document omitted.
  it.each([
    ["an empty locale projection", (m) => (m.notes_i18n.en = "")],
    ["an arbitrary string", (m) => (m.notes_i18n.en = "x")],
    [
      "notes replaced by the English projection",
      (m) => {
        m.notes = m.notes_i18n.en;
      },
    ],
  ])(
    "rejects a manifest with %s and no reviewed document",
    async (_label, mutate) => {
      const fixture = await releaseFixture();
      await rewriteManifest(fixture, mutate);
      await expect(
        verifyReleaseDirectory(fixture.root, fixture.version),
      ).rejects.toThrow(/notes_i18n|Release notes are invalid/);
    },
  );

  it("rejects missing, unexpected, checksum-drifted, and URL-drifted assets", async () => {
    const missing = await releaseFixture();
    await rm(join(missing.root, missing.names[0]));
    await expect(
      verifyReleaseDirectory(missing.root, missing.version),
    ).rejects.toThrow("inventory");

    const unexpected = await releaseFixture();
    await writeFile(join(unexpected.root, "extra.txt"), "unexpected");
    await expect(
      verifyReleaseDirectory(unexpected.root, unexpected.version),
    ).rejects.toThrow("inventory");

    const checksum = await releaseFixture();
    await writeFile(join(checksum.root, checksum.names[1]), "changed bytes");
    await expect(
      verifyReleaseDirectory(checksum.root, checksum.version),
    ).rejects.toThrow("does not match");

    const url = await releaseFixture();
    const manifestPath = join(url.root, "latest.json");
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    manifest.platforms["darwin-aarch64"].url =
      "https://example.invalid/AI.Router.app.tar.gz";
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    await expect(verifyReleaseDirectory(url.root, url.version)).rejects.toThrow(
      "canonical release assets",
    );
  });
});

describe("release application bundle inspection", () => {
  async function bundleFixture() {
    const bundle = await mkdtemp(join(tmpdir(), "ai-router-app-bundle-"));
    temporaryRoots.push(bundle);
    await mkdir(join(bundle, "Contents", "Resources"), { recursive: true });
    await Promise.all([
      writeFile(join(bundle, "Contents", "Resources", "LICENSE"), "MIT"),
      writeFile(
        join(bundle, "Contents", "Resources", "THIRD_PARTY_NOTICES.md"),
        "notices",
      ),
    ]);
    return bundle;
  }

  function inspectionRunner(executable = "ai-router-app") {
    const plist = {
      CFBundleExecutable: executable,
      CFBundleIdentifier: "com.relax.airouter",
      CFBundleName: "AI Router",
      CFBundleShortVersionString: "1.2.3",
      LSMinimumSystemVersion: "13.0",
    };
    return async (command, args) => {
      if (command === "/usr/libexec/PlistBuddy") {
        const key = args[1].replace("Print :", "");
        return { code: 0, stderr: "", stdout: `${plist[key]}\n` };
      }
      if (command === "lipo") {
        return { code: 0, stderr: "", stdout: "arm64\n" };
      }
      if (args[0] === "--verify") {
        return { code: 0, stderr: "", stdout: "" };
      }
      return {
        code: 0,
        stderr: "Signature=adhoc\nTeamIdentifier=not set\n",
        stdout: "",
      };
    };
  }

  it("accepts only the canonical application executable", async () => {
    const validBundle = await bundleFixture();
    await expect(
      inspectAppBundle(validBundle, "1.2.3", inspectionRunner()),
    ).resolves.toBeUndefined();

    const helperBundle = await bundleFixture();
    await expect(
      inspectAppBundle(
        helperBundle,
        "1.2.3",
        inspectionRunner("verify_update_signature"),
      ),
    ).rejects.toThrow("bundle metadata");
  });
});
