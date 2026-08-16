import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import {
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";

const EXPECTED_CODEX_VERSION = "codex-cli 0.147.0";
const EXPECTED_CODEX_SHA256 = new Map([
  [
    "darwin-arm64",
    "19c4f144c5226a9f17c58e6f0fa854843b0f77a6eb420f40e2745a12f10f5d37",
  ],
  [
    "darwin-x64",
    "8080a42da4cef9c4216dace512f29acfe2e526aeeec2a0ce450e5a2b18b84d8a",
  ],
]);
const PROCESS_TIMEOUT_MS = 60_000;
const codexPackageJson = new URL(
  "../node_modules/@openai/codex/package.json",
  import.meta.url,
);

async function sha256(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

async function codexBinaryPath() {
  const packageJson = JSON.parse(await readFile(codexPackageJson, "utf8"));
  if (packageJson.version !== "0.147.0") {
    throw new Error(
      `Codex retry contract expected package 0.147.0; found ${packageJson.version ?? "unavailable"}.`,
    );
  }
  const targets = new Map([
    ["darwin-arm64", ["aarch64-apple-darwin", "@openai/codex-darwin-arm64"]],
    ["darwin-x64", ["x86_64-apple-darwin", "@openai/codex-darwin-x64"]],
  ]);
  const platform = `${process.platform}-${process.arch}`;
  const target = targets.get(platform);
  if (!target) {
    throw new Error(
      `Codex retry contract has no reviewed binary for ${platform}.`,
    );
  }
  const [targetTriple, packageName] = target;
  const codexPackageRoot = await realpath(
    new URL("../node_modules/@openai/codex/", import.meta.url),
  );
  const packageRoot = await realpath(
    join(codexPackageRoot, "..", packageName.split("/").at(-1)),
  );
  return {
    expectedSha256: EXPECTED_CODEX_SHA256.get(platform),
    path: join(packageRoot, "vendor", targetTriple, "bin", "codex"),
  };
}

function runCodex(codexPath, args, options = {}, timeoutMs = null) {
  return new Promise((resolve, reject) => {
    const child = spawn(codexPath, args, {
      ...options,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timeoutId =
      timeoutMs === null
        ? null
        : setTimeout(() => {
            timedOut = true;
            child.kill("SIGTERM");
          }, timeoutMs);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      if (timeoutId !== null) clearTimeout(timeoutId);
      reject(error);
    });
    child.on("close", (code, signal) => {
      if (timeoutId !== null) clearTimeout(timeoutId);
      if (timedOut) {
        reject(new Error(`Codex probe exceeded ${timeoutMs} ms.`));
        return;
      }
      resolve({ code, signal, stdout, stderr });
    });
  });
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("The retry fixture did not bind an IPv4 loopback port.");
  }
  return address.port;
}

async function close(server) {
  await new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}

async function runFixture({
  codexPath,
  label,
  status,
  code,
  expectedRequests,
  responseBody = null,
  expectedOutputText = null,
}) {
  let requests = 0;
  let unexpectedRequest = null;
  const server = createServer((request, response) => {
    if (request.method !== "POST" || request.url !== "/v1/responses") {
      unexpectedRequest = `${request.method ?? "unknown"} ${request.url ?? "unknown"}`;
      response.writeHead(404, { connection: "close" });
      response.end();
      return;
    }
    requests += 1;
    request.resume();
    response.writeHead(status, {
      "content-type": "application/json",
      connection: "close",
    });
    response.end(
      JSON.stringify(
        responseBody ?? {
          error: {
            code,
            message: `Codex retry fixture ${label}`,
            type: code,
          },
        },
      ),
    );
  });
  const fixtureRoot = await mkdtemp(join(tmpdir(), "ai-router-codex-retry-"));
  try {
    const port = await listen(server);
    const codexHome = join(fixtureRoot, "codex-home");
    await mkdir(codexHome, { recursive: true });
    await writeFile(
      join(codexHome, "config.toml"),
      [
        'model = "gpt-5.1-codex-mini"',
        'model_provider = "retry_fixture"',
        'approval_policy = "never"',
        'sandbox_mode = "read-only"',
        "",
        "[model_providers.retry_fixture]",
        'name = "AI Router Codex Retry Fixture"',
        `base_url = "http://127.0.0.1:${port}/v1"`,
        'wire_api = "responses"',
        "requires_openai_auth = true",
        "supports_websockets = false",
        "stream_idle_timeout_ms = 300000",
        'experimental_bearer_token = "fixture-token"',
        "",
      ].join("\n"),
      "utf8",
    );

    const result = await runCodex(
      codexPath,
      [
        "exec",
        "--ephemeral",
        "--ignore-rules",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-C",
        fixtureRoot,
        "Return the word fixture.",
      ],
      {
        cwd: fixtureRoot,
        env: {
          ...process.env,
          CODEX_HOME: codexHome,
          CODEX_DISABLE_TELEMETRY: "1",
        },
      },
      PROCESS_TIMEOUT_MS,
    );
    if (result.code === 0) {
      throw new Error(`${label} fixture unexpectedly completed successfully.`);
    }
    if (unexpectedRequest !== null) {
      throw new Error(
        `${label} fixture used an unexpected upstream request: ${unexpectedRequest}.`,
      );
    }
    if (requests !== expectedRequests) {
      const diagnostic = result.stderr.trim().split("\n").slice(-4).join("\n");
      throw new Error(
        `${label} expected ${expectedRequests} router-visible request(s), observed ${requests}.\n${diagnostic}`,
      );
    }
    if (expectedOutputText !== null) {
      const output = `${result.stdout}\n${result.stderr}`;
      if (!output.includes(expectedOutputText)) {
        const diagnostic = output.trim().split("\n").slice(-8).join("\n");
        throw new Error(
          `${label} did not expose the expected normalized error text.\n${diagnostic}`,
        );
      }
    }
    console.log(`${label}: observed ${requests} request(s)`);
  } finally {
    if (server.listening) await close(server);
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

async function main() {
  const codexBinary = await codexBinaryPath();
  const actualSha256 = await sha256(codexBinary.path);
  if (actualSha256 !== codexBinary.expectedSha256) {
    throw new Error(
      `Codex retry contract expected binary SHA-256 ${codexBinary.expectedSha256}; found ${actualSha256}.`,
    );
  }
  const version = await runCodex(codexBinary.path, ["--version"]);
  const actualVersion = version.stdout.trim();
  if (version.code !== 0 || actualVersion !== EXPECTED_CODEX_VERSION) {
    throw new Error(
      `Codex retry contract is pinned to ${EXPECTED_CODEX_VERSION}; found ${actualVersion || "unavailable"}.`,
    );
  }
  await runFixture({
    codexPath: codexBinary.path,
    label: "retryable HTTP 500",
    status: 500,
    code: "server_error",
    expectedRequests: 30,
  });
  await runFixture({
    codexPath: codexBinary.path,
    label: "non-retryable HTTP 429",
    status: 429,
    code: "rate_limit_exceeded",
    expectedRequests: 1,
  });
  await runFixture({
    codexPath: codexBinary.path,
    label: "normalized upstream HTTP 403",
    status: 400,
    expectedRequests: 1,
    responseBody: {
      error: {
        code: "upstream_error",
        message:
          "Route 'fixture' returned HTTP 403 for model 'gpt-5.1-codex-mini'. Current user is in debt.",
        type: "invalid_request_error",
      },
      request_id: "fixture-request-id",
    },
    expectedOutputText: "Current user is in debt.",
  });
  console.log(`Codex retry compatibility passed: ${EXPECTED_CODEX_VERSION}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
