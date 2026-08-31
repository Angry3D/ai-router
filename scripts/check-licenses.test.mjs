import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import {
  checkThirdPartyProvenance,
  evaluateCargoMetadata,
  evaluatePnpmLicenses,
  runLicenseAudit,
  scanPublicTree,
  validateLicensePolicy,
} from "./check-licenses.mjs";

const projectRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const temporaryRoots = [];

async function temporaryRoot(prefix) {
  const root = await mkdtemp(join(tmpdir(), prefix));
  temporaryRoots.push(root);
  return root;
}

afterEach(async () => {
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { force: true, recursive: true })),
  );
});

describe("license and provenance gate", () => {
  it("accepts the repository project metadata and required provenance bindings", async () => {
    const tauriConfig = JSON.parse(
      await readFile(join(projectRoot, "src-tauri", "tauri.conf.json"), "utf8"),
    );
    const report = await runLicenseAudit({
      projectRoot,
      skipDependencies: true,
    });

    expect(report.project).toEqual({
      license: "MIT",
      name: "ai-router",
      version: tauriConfig.version,
    });
    expect(report.thirdParty.map((entry) => entry.id)).toEqual([
      "openai-codex-base-instructions",
      "lucide-icons",
      "readme-product-screenshots",
      "openai-pricing-snapshots",
    ]);
  });

  it("rejects a live application version in the license policy", async () => {
    const policy = JSON.parse(
      await readFile(
        join(projectRoot, "scripts", "license-policy.json"),
        "utf8",
      ),
    );
    policy.project.version = "9.9.9";

    expect(() => validateLicensePolicy(policy)).toThrow(
      "project.version must be derived from the Tauri manifest",
    );
  });

  it("validates the Cargo probe before starting metadata", async () => {
    const root = await temporaryRoot("ai-router-license-policy-");
    const policy = JSON.parse(
      await readFile(
        join(projectRoot, "scripts", "license-policy.json"),
        "utf8",
      ),
    );
    policy.tools.node = process.version.replace(/^v/, "");
    const policyPath = join(root, "license-policy.json");
    await writeFile(policyPath, JSON.stringify(policy));

    let signalCargoProbeStarted;
    const cargoProbeStarted = new Promise((resolvePromise) => {
      signalCargoProbeStarted = resolvePromise;
    });
    let resolveCargoProbe;
    const cargoProbe = new Promise((resolvePromise) => {
      resolveCargoProbe = resolvePromise;
    });
    let metadataStarted = false;
    const commandRunner = (command, args) => {
      if (command === "cargo" && args[0] === "--version") {
        signalCargoProbeStarted();
        return cargoProbe;
      }
      if (command === "cargo" && args[0] === "metadata") {
        metadataStarted = true;
        return Promise.resolve("{}");
      }
      if (command === "pnpm" && args[0] === "--version") {
        return Promise.resolve(policy.tools.pnpm);
      }
      if (command === "pnpm" && args[0] === "licenses") {
        return Promise.resolve("{}");
      }
      throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
    };

    const audit = runLicenseAudit({ commandRunner, policyPath, projectRoot });
    await cargoProbeStarted;
    expect(metadataStarted).toBe(false);

    resolveCargoProbe("cargo 0.0.0 (fixture)");
    await expect(audit).rejects.toThrow("does not match pinned Cargo 1.97.1");
    expect(metadataStarted).toBe(false);
  });

  it("rejects a missing required NOTICE entry", async () => {
    const root = await temporaryRoot("ai-router-provenance-");
    await mkdir(join(root, "embedded"));
    await Promise.all([
      writeFile(join(root, "THIRD_PARTY_NOTICES.md"), "# Notices\n"),
      writeFile(join(root, "embedded", "content.txt"), "upstream content\n"),
      writeFile(join(root, "embedded", "SOURCE.md"), "upstream-v1 MIT\n"),
    ]);
    const policy = {
      pricingCatalogs: [],
      thirdParty: [
        {
          hashes: {},
          id: "upstream-content",
          license: "MIT",
          localPaths: ["embedded/content.txt", "embedded/SOURCE.md"],
          name: "Upstream content",
          noticeMarker: "<!-- provenance:upstream-content -->",
          sourceRecord: {
            path: "embedded/SOURCE.md",
            requiredStrings: ["upstream-v1", "MIT"],
          },
          upstream: "https://example.invalid/upstream",
          version: "upstream-v1",
        },
      ],
    };

    await expect(
      checkThirdPartyProvenance(
        root,
        policy,
        await readFile(join(root, "THIRD_PARTY_NOTICES.md"), "utf8"),
      ),
    ).rejects.toThrow("missing upstream-content");
  });

  it("accepts reviewed compound dependency licenses and strips local package paths", () => {
    const report = evaluatePnpmLicenses(
      {
        "Apache-2.0 OR MIT": [
          {
            license: "Apache-2.0 OR MIT",
            name: "dual-license",
            paths: ["/private-license-root/node_modules/dual-license"],
            versions: ["1.2.3"],
          },
        ],
      },
      new Set(["Apache-2.0", "MIT"]),
    );

    expect(report).toEqual({
      packageCount: 1,
      packages: [
        {
          license: "Apache-2.0 OR MIT",
          name: "dual-license",
          version: "1.2.3",
        },
      ],
    });
    expect(JSON.stringify(report)).not.toContain("/private-license-root/");
  });

  it("fails closed on unknown or unreviewed JavaScript licenses", () => {
    expect(() =>
      evaluatePnpmLicenses(
        { UNKNOWN: [{ name: "missing", versions: ["1.0.0"] }] },
        new Set(["MIT"]),
      ),
    ).toThrow("Missing, unknown");
    expect(() =>
      evaluatePnpmLicenses(
        { "GPL-3.0-only": [{ name: "copyleft", versions: ["1.0.0"] }] },
        new Set(["MIT"]),
      ),
    ).toThrow("GPL-3.0-only");
  });

  it("checks every Cargo package and workspace repository boundary", async () => {
    const metadata = {
      packages: [
        {
          license: "MIT",
          name: "ai-router",
          repository: "https://github.com/Angry3D/ai-router",
          source: null,
          version: "0.1.0",
        },
        {
          license: "MIT OR Apache-2.0",
          name: "dependency",
          repository: "https://example.invalid/dependency",
          source: "registry+https://github.com/rust-lang/crates.io-index",
          version: "2.0.0",
        },
        {
          license: "MIT OR LGPL-2.1-or-later",
          name: "choice-dependency",
          repository: "https://example.invalid/choice-dependency",
          source: "registry+https://github.com/rust-lang/crates.io-index",
          version: "3.0.0",
        },
      ],
    };

    await expect(
      evaluateCargoMetadata(
        metadata,
        new Set(["Apache-2.0", "MIT"]),
        "https://github.com/Angry3D/ai-router",
      ),
    ).resolves.toMatchObject({ packageCount: 3 });
    await expect(
      evaluateCargoMetadata(
        { packages: [{ ...metadata.packages[1], license: null }] },
        new Set(["Apache-2.0", "MIT"]),
        "https://github.com/Angry3D/ai-router",
      ),
    ).rejects.toThrow("Missing, unknown");
  });

  it("binds a missing Cargo license field to one exact audited override", async () => {
    const root = await temporaryRoot("ai-router-cargo-license-");
    const licenseText = "MIT fixture license\n";
    const source = "git+https://example.invalid/dependency#abc123";
    await Promise.all([
      writeFile(join(root, "Cargo.toml"), '[package]\nname = "dependency"\n'),
      writeFile(join(root, "LICENSE_MIT"), licenseText),
    ]);
    const override = {
      disposition: "Fixture manifest omits its license field.",
      license: "MIT",
      licenseFiles: [
        {
          path: "LICENSE_MIT",
          sha256: createHash("sha256").update(licenseText).digest("hex"),
        },
      ],
      name: "dependency",
      source,
      version: "1.0.0",
    };
    const metadata = {
      packages: [
        {
          license: null,
          manifest_path: join(root, "Cargo.toml"),
          name: "dependency",
          repository: "https://example.invalid/dependency",
          source,
          version: "1.0.0",
        },
      ],
    };

    await expect(
      evaluateCargoMetadata(metadata, new Set(["MIT"]), "unused", [override]),
    ).resolves.toMatchObject({
      packages: [{ license: "MIT", licenseSource: "policy-override" }],
    });
    await writeFile(join(root, "LICENSE_MIT"), "changed\n");
    await expect(
      evaluateCargoMetadata(metadata, new Set(["MIT"]), "unused", [override]),
    ).rejects.toThrow("license override hash changed");
  });

  it("rejects private workflow markers and undeclared visual assets in a public tree", async () => {
    const root = await temporaryRoot("ai-router-public-tree-");
    await mkdir(join(root, "src"));
    await writeFile(
      join(root, "src", "main.ts"),
      "export const ready = true;\n",
    );
    const policy = {
      declaredVisualAssets: [],
      declaredVisualAssetHashes: {},
      publicTree: {
        forbiddenExactPaths: [],
        forbiddenPathPrefixes: [".trellis/"],
        forbiddenTextMarkers: ["<!-- TRELLIS:START -->"],
        markerExemptPaths: [],
      },
    };

    await expect(scanPublicTree(root, policy)).resolves.toEqual({
      fileCount: 1,
      visualAssetCount: 0,
    });
    await writeFile(
      join(root, "src", "private.ts"),
      "/* <!-- TRELLIS:START --> */\n",
    );
    await expect(scanPublicTree(root, policy)).rejects.toThrow(
      "forbidden template marker",
    );
    await rm(join(root, "src", "private.ts"));
    await writeFile(
      join(root, "src", "capture.png"),
      Buffer.from([137, 80, 78, 71]),
    );
    await expect(scanPublicTree(root, policy)).rejects.toThrow(
      "undeclared visual asset",
    );
  });

  it("allows the license checker to define the reviewed forbidden markers", async () => {
    const root = await temporaryRoot("ai-router-license-checker-");
    await mkdir(join(root, "scripts"));
    await writeFile(
      join(root, "scripts", "check-licenses.mjs"),
      'const marker = "<!-- TRELLIS:START -->";\n',
    );
    const policy = validateLicensePolicy(
      JSON.parse(
        await readFile(
          join(projectRoot, "scripts", "license-policy.json"),
          "utf8",
        ),
      ),
    );

    await expect(scanPublicTree(root, policy)).resolves.toMatchObject({
      fileCount: 1,
    });
  });

  it("rejects changed bytes for a declared visual asset", async () => {
    const root = await temporaryRoot("ai-router-public-visual-");
    await mkdir(join(root, "src"));
    const path = "src/reviewed.svg";
    const reviewed = Buffer.from("<svg/>\n");
    await writeFile(join(root, path), reviewed);
    const policy = {
      declaredVisualAssets: [path],
      declaredVisualAssetHashes: {
        [path]: createHash("sha256").update(reviewed).digest("hex"),
      },
      publicTree: {
        forbiddenExactPaths: [],
        forbiddenPathPrefixes: [],
        forbiddenTextMarkers: [],
        markerExemptPaths: [],
      },
    };

    await expect(scanPublicTree(root, policy)).resolves.toMatchObject({
      visualAssetCount: 1,
    });
    await writeFile(join(root, path), "<svg><text>changed</text></svg>\n");
    await expect(scanPublicTree(root, policy)).rejects.toThrow(
      "Public visual asset hash changed",
    );
  });

  it("does not hide nested tracked directories that resemble build output", async () => {
    const root = await temporaryRoot("ai-router-public-nested-target-");
    await mkdir(join(root, "src", "target"), { recursive: true });
    await writeFile(
      join(root, "src", "target", "private.ts"),
      "/* <!-- TRELLIS:START --> */\n",
    );
    const policy = {
      declaredVisualAssets: [],
      declaredVisualAssetHashes: {},
      publicTree: {
        forbiddenExactPaths: [],
        forbiddenPathPrefixes: [],
        forbiddenTextMarkers: ["<!-- TRELLIS:START -->"],
        markerExemptPaths: [],
      },
    };

    await expect(scanPublicTree(root, policy)).rejects.toThrow(
      "forbidden template marker",
    );
  });

  it("requires every declared visual asset to have a provenance record", () => {
    expect(() =>
      validateLicensePolicy({
        declaredVisualAssets: ["images/capture.png"],
        declaredVisualAssetHashes: {},
        dependencies: {
          javascriptAllowedIdentifiers: ["MIT"],
          rustAllowedIdentifiers: ["MIT"],
          rustLicenseOverrides: [],
        },
        policyVersion: "test",
        pricingCatalogs: [],
        project: {},
        publicTree: {},
        schemaVersion: 1,
        thirdParty: [],
        tools: {},
      }),
    ).toThrow("lacks a third-party provenance record");
  });

  it("rejects attempts to weaken the reviewed public-tree boundary", async () => {
    const policy = JSON.parse(
      await readFile(
        join(projectRoot, "scripts", "license-policy.json"),
        "utf8",
      ),
    );
    policy.publicTree.forbiddenPathPrefixes =
      policy.publicTree.forbiddenPathPrefixes.filter(
        (prefix) => prefix !== ".trellis/",
      );

    expect(() => validateLicensePolicy(policy)).toThrow(
      "forbiddenPathPrefixes drifted from the reviewed public-tree boundary",
    );
  });
});
