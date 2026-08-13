import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checkedDirectory = join(projectRoot, "src", "generated");
const temporaryRoot = mkdtempSync(join(tmpdir(), "ai-router-types-"));
const generatedDirectory = join(temporaryRoot, "generated");
const checkOnly = process.argv.includes("--check");
const apiKeyDtoAllowlist = new Set([
  "BalanceTestInputDto.ts",
  "RouteEditDto.ts",
  "RouteSaveInputDto.ts",
]);

function listFiles(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory, { withFileTypes: true })
    .flatMap((entry) => {
      const path = join(directory, entry.name);
      return entry.isDirectory() ? listFiles(path) : [relative(directory, path)];
    })
    .sort();
}

try {
  mkdirSync(generatedDirectory, { recursive: true });
  const generation = spawnSync(
    "cargo",
    ["run", "--quiet", "-p", "router-core", "--bin", "generate-types", "--", generatedDirectory],
    { cwd: projectRoot, encoding: "utf8", stdio: "pipe" },
  );

  if (generation.status !== 0) {
    process.stderr.write(generation.stderr || generation.stdout);
    process.exit(generation.status ?? 1);
  }

  const typeFiles = listFiles(generatedDirectory).filter((file) => file.endsWith(".ts"));
  const exports = typeFiles
    .filter((file) => file !== "index.ts")
    .map((file) => {
      const typeName = file.slice(0, -3);
      return `export type { ${typeName} } from "./${typeName}";`;
    });
  writeFileSync(join(generatedDirectory, "index.ts"), `${exports.join("\n")}\n`);

  const unexpectedApiKeyDtos = typeFiles.filter(
    (file) =>
      readFileSync(join(generatedDirectory, file), "utf8").includes("apiKey") &&
      !apiKeyDtoAllowlist.has(file),
  );
  if (unexpectedApiKeyDtos.length > 0) {
    process.stderr.write(
      `Generated DTOs expose apiKey outside the authorized edit/input boundary: ${unexpectedApiKeyDtos.join(", ")}\n`,
    );
    process.exit(1);
  }

  if (!checkOnly) {
    rmSync(checkedDirectory, { force: true, recursive: true });
    cpSync(generatedDirectory, checkedDirectory, { recursive: true });
    process.stdout.write(`Generated ${listFiles(checkedDirectory).length} TypeScript files.\n`);
    process.exit(0);
  }

  const expectedFiles = listFiles(generatedDirectory);
  const actualFiles = listFiles(checkedDirectory);
  if (JSON.stringify(expectedFiles) !== JSON.stringify(actualFiles)) {
    process.stderr.write(
      `Generated type file set differs. Expected [${expectedFiles.join(", ")}], found [${actualFiles.join(", ")}].\n`,
    );
    process.exit(1);
  }

  for (const file of expectedFiles) {
    const expected = readFileSync(join(generatedDirectory, file));
    const actual = readFileSync(join(checkedDirectory, file));
    if (!expected.equals(actual)) {
      process.stderr.write(`Generated type differs: ${file}\n`);
      process.exit(1);
    }
  }

  process.stdout.write(`Generated type check passed for ${expectedFiles.length} files.\n`);
} finally {
  rmSync(temporaryRoot, { force: true, recursive: true });
}
