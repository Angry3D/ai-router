import { cp, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

const ICONS = [
  {
    source: "src-tauri/icons/app-icon.svg",
    target: "src-tauri/icons/app-icon",
  },
  {
    source: "src-tauri/icons/app-icon-qa.svg",
    target: "src-tauri/icons/app-icon-qa",
  },
];

const TRAY_ICONS = [
  "tray-active-static",
  "tray-active-a",
  "tray-active-b",
  "tray-active-c",
  "tray-active-d",
];

function runTauriIcon(source, output) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(
      tauriCommand,
      ["exec", "tauri", "icon", source, "--output", output],
      { cwd: projectRoot, stdio: "inherit" },
    );
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`Tauri icon generation failed for ${source}.`));
    });
  });
}

function runCommand(command, args) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { cwd: projectRoot, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} exited with code ${code}.`));
    });
  });
}

async function readOptional(path) {
  try {
    return await readFile(path);
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return null;
    }
    throw error;
  }
}

async function generateIcon({ source, target }) {
  const output = await mkdtemp(join(tmpdir(), "ai-router-icon-"));
  try {
    await runTauriIcon(join(projectRoot, source), output);
    const generatedPngPath = join(output, "icon.png");
    const targetPngPath = join(projectRoot, `${target}.png`);
    const targetIcnsPath = join(projectRoot, `${target}.icns`);
    const [generatedPng, targetPng, targetIcns] = await Promise.all([
      readFile(generatedPngPath),
      readOptional(targetPngPath),
      readOptional(targetIcnsPath),
    ]);
    const assetsAreCurrent =
      targetPng?.equals(generatedPng) &&
      targetIcns?.subarray(0, 4).toString("ascii") === "icns";
    if (!assetsAreCurrent) {
      await Promise.all([
        cp(join(output, "icon.icns"), targetIcnsPath),
        cp(generatedPngPath, targetPngPath),
      ]);
    }
  } finally {
    await rm(output, { force: true, recursive: true });
  }
}

async function generateTrayIcon(name) {
  if (process.platform !== "darwin") {
    throw new Error("Tray template generation requires macOS sips.");
  }
  const output = await mkdtemp(join(tmpdir(), "ai-router-tray-icon-"));
  try {
    await runTauriIcon(
      join(projectRoot, `src-tauri/icons/${name}.svg`),
      output,
    );
    await runCommand("/usr/bin/sips", [
      "-z",
      "44",
      "44",
      join(output, "icon.png"),
      "--out",
      join(projectRoot, `src-tauri/icons/${name}.png`),
    ]);
  } finally {
    await rm(output, { force: true, recursive: true });
  }
}

for (const icon of ICONS) {
  await generateIcon(icon);
}

for (const icon of TRAY_ICONS) {
  await generateTrayIcon(icon);
}

await cp(
  join(projectRoot, "src-tauri/icons/app-icon.png"),
  join(projectRoot, "src-tauri/icons/icon.png"),
);

console.log("Generated production, QA, and four-frame active tray icons.");
