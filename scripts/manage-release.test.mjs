import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  createLatestManifest,
  expectedAssetNames,
  inspectAppBundle,
  ReleaseError,
  releaseNotes,
  renderReleaseConfig,
  uploadPreparedDraft,
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
  const manifest = createLatestManifest(
    version,
    "2026-08-16T10:00:00.000Z",
    signature,
  );
  const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
  content.set("latest.json", manifestBytes);
  await writeFile(join(root, "latest.json"), manifestBytes);
  const checksums = names
    .slice(0, 4)
    .map((name) => `${digest(content.get(name))}  ${name}`)
    .join("\n");
  await writeFile(join(root, "SHA256SUMS"), `${checksums}\n`);
  return { names, root, version };
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

describe("release metadata and inventory", () => {
  it("produces user-facing DMG guidance without claiming Apple verification", () => {
    const notes = releaseNotes("1.2.3");
    expect(notes).toContain("首次安装请下载 DMG");
    expect(notes).toContain("未经 Apple Developer ID 验证或公证");
  });

  it("accepts one complete internally consistent release directory", async () => {
    const fixture = await releaseFixture();
    await expect(
      verifyReleaseDirectory(fixture.root, fixture.version),
    ).resolves.toMatchObject({ assets: fixture.names });
  });

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
