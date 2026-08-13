import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rename,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";

export const QA_IDENTIFIER = "com.relax.airouter.qa";
export const PRODUCTION_IDENTIFIER = "com.relax.airouter";
export const QA_BUNDLE_NAME = "AI Router QA.app";
export const QA_ACCEPTANCE_ROOT_ENV = "AI_ROUTER_QA_ACCEPTANCE_ROOT";
export const QA_ACCEPTANCE_ROOT_PREFIX = "ai-router-v0-2a-qa-";
export const QA_ACCEPTANCE_MARKER_FILE = ".ai-router-qa-acceptance-root";
export const QA_RUNTIME_MARKER_FILE = "runtime-marker.json";

export class QaAcceptanceError extends Error {}

export async function createRunRoot(temporaryDirectory = tmpdir()) {
  const nonce = randomUUID();
  const root = await mkdtemp(
    join(temporaryDirectory, `${QA_ACCEPTANCE_ROOT_PREFIX}${nonce}-`),
  );
  const resolved = await realpath(root);
  const rootNonce = basename(resolved).slice(QA_ACCEPTANCE_ROOT_PREFIX.length);
  await Promise.all([
    mkdir(join(resolved, "app-data"), { recursive: true, mode: 0o700 }),
    mkdir(join(resolved, "logs"), { recursive: true, mode: 0o700 }),
    writeFile(join(resolved, QA_ACCEPTANCE_MARKER_FILE), rootNonce, {
      encoding: "utf8",
      mode: 0o600,
    }),
  ]);
  return { nonce: rootNonce, root: resolved };
}

export async function resolveRunRoot(candidate, temporaryDirectory = tmpdir()) {
  if (typeof candidate !== "string" || !isAbsolute(candidate)) {
    throw new QaAcceptanceError("QA acceptance root must be an absolute path.");
  }
  if (
    [...candidate].some((character) => {
      const code = character.charCodeAt(0);
      return code <= 0x1f || code === 0x7f;
    })
  ) {
    throw new QaAcceptanceError(
      "QA acceptance root contains a control character.",
    );
  }
  const metadata = await lstat(candidate);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new QaAcceptanceError("QA acceptance root must be a real directory.");
  }
  const [root, temporaryRoot] = await Promise.all([
    realpath(candidate),
    realpath(temporaryDirectory),
  ]);
  if (root === temporaryRoot || !root.startsWith(`${temporaryRoot}${sep}`)) {
    throw new QaAcceptanceError(
      "QA acceptance root is outside the OS temporary directory.",
    );
  }
  const name = basename(root);
  const nonce = name.startsWith(QA_ACCEPTANCE_ROOT_PREFIX)
    ? name.slice(QA_ACCEPTANCE_ROOT_PREFIX.length)
    : "";
  if (!nonce || nonce.length > 64 || !/^[A-Za-z0-9-]+$/u.test(nonce)) {
    throw new QaAcceptanceError("QA acceptance root name is invalid.");
  }
  const marker = join(root, QA_ACCEPTANCE_MARKER_FILE);
  const markerMetadata = await lstat(marker).catch(() => null);
  if (!markerMetadata?.isFile() || markerMetadata.isSymbolicLink()) {
    throw new QaAcceptanceError("QA acceptance root marker is invalid.");
  }
  const markerNonce = (await readFile(marker, "utf8")).trim();
  if (markerNonce !== nonce) {
    throw new QaAcceptanceError("QA acceptance root marker does not match.");
  }
  return { nonce, root };
}

export async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

export async function writeJsonAtomically(path, value) {
  await writeTextAtomically(path, `${JSON.stringify(value, null, 2)}\n`);
}

export async function writeTextAtomically(path, value) {
  const destination = resolve(path);
  await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
  const temporary = `${destination}.${process.pid}.${randomUUID()}.tmp`;
  await writeFile(temporary, value, {
    encoding: "utf8",
    flag: "wx",
    mode: 0o600,
  });
  await rename(temporary, destination);
}

export function assertExactKeys(record, allowedKeys, label) {
  if (record === null || Array.isArray(record) || typeof record !== "object") {
    throw new QaAcceptanceError(`${label} must be an object.`);
  }
  const allowed = new Set(allowedKeys);
  const unexpected = Object.keys(record).filter((key) => !allowed.has(key));
  if (unexpected.length > 0) {
    throw new QaAcceptanceError(
      `${label} contains non-allowlisted fields: ${unexpected.join(", ")}.`,
    );
  }
}

export function assertLoopbackUrl(value, label = "URL") {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new QaAcceptanceError(`${label} is invalid.`);
  }
  if (parsed.protocol !== "http:" || parsed.hostname !== "127.0.0.1") {
    throw new QaAcceptanceError(`${label} must use IPv4 loopback HTTP.`);
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new QaAcceptanceError(`${label} contains forbidden URL components.`);
  }
  return parsed;
}

export function runCommand(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.once("error", reject);
    child.once("close", (code, signal) => {
      resolvePromise({ code, signal, stderr, stdout });
    });
  });
}

export function optionValue(arguments_, name) {
  const index = arguments_.indexOf(name);
  if (index === -1 || index + 1 >= arguments_.length) {
    throw new QaAcceptanceError(`Missing required option ${name}.`);
  }
  return arguments_[index + 1];
}
