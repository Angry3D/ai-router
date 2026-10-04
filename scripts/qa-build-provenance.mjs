import { createHash, randomUUID } from "node:crypto";
import { createReadStream } from "node:fs";
import { lstat, readFile, realpath, rm } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

import {
  QA_BUNDLE_NAME,
  QA_IDENTIFIER,
  QaAcceptanceError,
  runCommand,
  writeJsonAtomically,
} from "./v0-2a-qa-common.mjs";

const RECEIPT_KEYS = [
  "schemaVersion",
  "buildId",
  "revision",
  "sourceFingerprint",
  "completedAtMs",
  "bundlePath",
  "identifier",
  "bundleName",
  "version",
  "executablePath",
  "executableSha256",
];
const SHA256 = /^[a-f0-9]{64}$/u;
// Build inputs only: public documentation and private runtime/task trees are not
// sources. fixtures includes the Markdown instructions embedded by router-core.
const INPUT_TREES = [
  "src/",
  "src-tauri/",
  "crates/",
  "fixtures/",
  "scripts/",
  "public/",
];
const INPUT_FILES = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
  "rust-toolchain.toml",
  ".nvmrc",
  ".gitignore",
  ".gitattributes",
  ".cargo/config",
  ".cargo/config.toml",
]);

export function qaBuildPaths(root) {
  return {
    bundlePath: resolve(root, "target/release/bundle/macos", QA_BUNDLE_NAME),
    receiptPath: resolve(root, "target/release/qa-build-receipt.json"),
  };
}

function isBuildInput(path) {
  if (
    path
      .split("/")
      .some((part) => ["target", "node_modules", "dist"].includes(part)) ||
    path.startsWith("src-tauri/gen/") ||
    path.endsWith(".tsbuildinfo") ||
    path.endsWith(".log") ||
    path.endsWith(".DS_Store")
  )
    return false;
  if (INPUT_FILES.has(path)) return true;
  if (!path.includes("/")) {
    return /^(?:tsconfig(?:\.[\w-]+)?\.json|vite\.config\.[cm]?[jt]s|[\w-]+\.html)$/u.test(
      path,
    );
  }
  if (!INPUT_TREES.some((tree) => path.startsWith(tree))) return false;
  if (path.endsWith(".md") && !path.startsWith("fixtures/")) return false;
  return !path.startsWith("scripts/") || !path.endsWith(".test.mjs");
}

async function gitOutput(root, args) {
  const result = await runCommand("git", args, { cwd: root });
  if (result.code !== 0) {
    throw new QaAcceptanceError("Unable to fingerprint QA source inputs.");
  }
  return result.stdout;
}

async function fileSha256(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

export async function captureQaSource(root) {
  try {
    const sourceRoot = await realpath(root);
    const gitRoot = (
      await gitOutput(sourceRoot, ["rev-parse", "--show-toplevel"])
    ).trim();
    if ((await realpath(gitRoot)) !== sourceRoot) {
      throw new QaAcceptanceError("QA source must be the repository root.");
    }
    const revision = (
      await gitOutput(sourceRoot, ["rev-parse", "HEAD"])
    ).trim();
    const paths = [
      ...new Set(
        (
          await gitOutput(sourceRoot, [
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
          ])
        )
          .split("\0")
          .filter((path) => path && isBuildInput(path)),
      ),
    ].sort();
    const hash = createHash("sha256");
    for (const path of paths) {
      const absolute = join(sourceRoot, path);
      const metadata = await lstat(absolute).catch((error) => {
        if (error.code === "ENOENT") return null;
        throw error;
      });
      // Do not follow symlinks into private or ignored files outside the input set.
      if (metadata !== null && !metadata.isFile()) {
        throw new QaAcceptanceError("QA source inputs must be regular files.");
      }
      hash.update(
        JSON.stringify([
          path,
          metadata === null ? "deleted" : metadata.mode & 0o111,
          metadata === null ? null : await fileSha256(absolute),
        ]) + "\n",
      );
    }
    return { revision, sourceFingerprint: hash.digest("hex") };
  } catch (error) {
    if (error instanceof QaAcceptanceError) throw error;
    throw new QaAcceptanceError("Unable to fingerprint QA source inputs.");
  }
}

function sameSource(left, right) {
  return (
    left.revision === right.revision &&
    left.sourceFingerprint === right.sourceFingerprint
  );
}

// Only the build owner calls this transaction. Inspection can never issue a
// receipt for an old binary: evidence is removed before even reading sources.
export async function withQaBuildReceipt(root, build, inspectBundle) {
  const { receiptPath } = qaBuildPaths(root);
  await rm(receiptPath, { force: true });
  const sourceRoot = await realpath(root);
  const source = await captureQaSource(root);
  await build();
  const bundle = await inspectBundle();
  const executableSha256 = await fileSha256(bundle.executablePath);
  if (!sameSource(source, await captureQaSource(root))) {
    throw new QaAcceptanceError(
      "QA source changed during the build; rebuild required.",
    );
  }
  const receipt = {
    schemaVersion: 1,
    buildId: randomUUID(),
    ...source,
    completedAtMs: Date.now(),
    bundlePath: relative(sourceRoot, bundle.bundlePath),
    identifier: bundle.identifier,
    bundleName: bundle.bundleName,
    version: bundle.version,
    executablePath: relative(sourceRoot, bundle.executablePath),
    executableSha256,
  };
  await writeJsonAtomically(receiptPath, receipt);
}

export async function verifyQaBuildReceipt(root, bundle) {
  let receipt;
  try {
    const sourceRoot = await realpath(root);
    const { receiptPath, bundlePath } = qaBuildPaths(root);
    const metadata = await lstat(receiptPath);
    if (!metadata.isFile() || (metadata.mode & 0o077) !== 0) {
      throw new Error("invalid receipt file");
    }
    receipt = JSON.parse(await readFile(receiptPath, "utf8"));
    if (
      receipt === null ||
      typeof receipt !== "object" ||
      Object.keys(receipt).length !== RECEIPT_KEYS.length ||
      !RECEIPT_KEYS.every((key) => Object.hasOwn(receipt, key)) ||
      receipt.schemaVersion !== 1 ||
      !/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/u.test(receipt.buildId) ||
      !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/u.test(receipt.revision) ||
      !SHA256.test(receipt.sourceFingerprint) ||
      !SHA256.test(receipt.executableSha256) ||
      !Number.isSafeInteger(receipt.completedAtMs) ||
      receipt.completedAtMs <= 0 ||
      receipt.completedAtMs > Date.now() ||
      typeof receipt.version !== "string" ||
      receipt.version.length === 0 ||
      receipt.identifier !== QA_IDENTIFIER ||
      receipt.bundleName !== "AI Router QA" ||
      bundle.bundlePath !== (await realpath(bundlePath)) ||
      receipt.bundlePath !== relative(sourceRoot, bundle.bundlePath) ||
      receipt.executablePath !== relative(sourceRoot, bundle.executablePath) ||
      receipt.identifier !== bundle.identifier ||
      receipt.bundleName !== bundle.bundleName ||
      receipt.version !== bundle.version
    )
      throw new Error("invalid receipt");
  } catch {
    throw new QaAcceptanceError(
      "QA build receipt is missing or invalid; rebuild required.",
    );
  }
  if (!sameSource(receipt, await captureQaSource(root))) {
    throw new QaAcceptanceError("QA build source is stale; rebuild required.");
  }
  let digest;
  try {
    digest = await fileSha256(bundle.executablePath);
  } catch {
    throw new QaAcceptanceError("Unable to verify the QA build executable.");
  }
  if (digest !== receipt.executableSha256) {
    throw new QaAcceptanceError(
      "QA build executable does not match its receipt; rebuild required.",
    );
  }
  return receipt;
}
