import { appendFile, unlink } from "node:fs/promises";
import { createServer } from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  QaAcceptanceError,
  assertExactKeys,
  assertLoopbackUrl,
  createRunRoot,
  optionValue,
  readJson,
  resolveRunRoot,
  writeJsonAtomically,
} from "./v0-2a-qa-common.mjs";

const ROUTE_LABELS = ["A", "B", "C", "D"];
const ALLOWED_SCENARIOS = new Set([
  "success-json",
  "success-sse",
  "http-500",
  "http-429",
  "account-error",
  "header-delay",
  "connection-close",
  "lifecycle-only",
  "meaningful-delay",
  "meaningful-pending",
  "meaningful-close",
  "terminal-completed",
  "terminal-done",
  "terminal-pending",
  "pending",
]);
const EVENT_KEYS = [
  "schemaVersion",
  "sequence",
  "routeLabel",
  "requestKind",
  "scenario",
  "phase",
  "statusClass",
  "observedAtMs",
  "clientClosed",
];

function routeKind(pathname) {
  const match = /^\/v1\/(responses|usage)$/u.exec(pathname);
  return match?.[1] ?? null;
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new QaAcceptanceError("QA fixture did not bind IPv4 loopback.");
  }
  return address.port;
}

async function closeServer(server) {
  await new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}

function sendJson(response, status, value) {
  response.writeHead(status, {
    "content-type": "application/json",
    connection: "close",
  });
  response.end(JSON.stringify(value));
}

function terminalEvent() {
  return 'data: {"type":"response.completed","response":{"status":"completed"}}\n\n';
}

function meaningfulEvent() {
  return 'data: {"type":"response.output_text.delta","delta":"fixture"}\n\n';
}

function safeServer(handler) {
  return createServer((request, response) => {
    void handler(request, response).catch(() => {
      if (response.headersSent) response.destroy();
      else sendJson(response, 500, { error: "fixture_internal_error" });
    });
  });
}

export async function createFixtureServer({ root: candidateRoot }) {
  const { nonce, root } = await resolveRunRoot(candidateRoot);
  const ledgerPath = join(root, "fixture-events.sanitized.jsonl");
  await unlink(ledgerPath).catch((error) => {
    if (error.code !== "ENOENT") throw error;
  });

  const controls = new Map(
    ROUTE_LABELS.map((label) => [
      label,
      { delayMs: 0, scenario: "success-json" },
    ]),
  );
  const counters = new Map(
    ROUTE_LABELS.map((label) => [label, { responses: 0, usage: 0 }]),
  );
  const timers = new Set();
  const responses = new Set();
  const servers = [];
  const fixtureStartedAt = process.hrtime.bigint();
  let unexpectedTrafficCount = 0;
  let sequence = 0;
  let ledgerWrite = Promise.resolve();
  let closePromise;

  const observedAtMs = () =>
    Number((process.hrtime.bigint() - fixtureStartedAt) / 1_000_000n);

  const record = (event) => {
    const safe = { schemaVersion: 1, sequence: ++sequence, ...event };
    assertExactKeys(safe, EVENT_KEYS, "fixture event");
    ledgerWrite = ledgerWrite.then(() =>
      appendFile(ledgerPath, `${JSON.stringify(safe)}\n`, "utf8"),
    );
    return ledgerWrite;
  };

  const rejectUnexpected = async (request, response, routeLabel = null) => {
    unexpectedTrafficCount += 1;
    request.resume();
    await record({
      routeLabel,
      requestKind: "unexpected",
      scenario: "unexpected",
      phase: "received",
      statusClass: "4xx",
      observedAtMs: observedAtMs(),
      clientClosed: false,
    });
    sendJson(response, 404, { error: "fixture_not_found" });
  };

  const handleRoute = async (label, request, response) => {
    const parsed = new URL(request.url ?? "/", "http://127.0.0.1");
    const kind = routeKind(parsed.pathname);
    if (!kind || !["GET", "POST"].includes(request.method ?? "")) {
      await rejectUnexpected(request, response, label);
      return;
    }

    request.resume();
    const routeCounters = counters.get(label);
    routeCounters[kind] += 1;
    const control = controls.get(label);
    await record({
      routeLabel: label,
      requestKind: kind,
      scenario: control.scenario,
      phase: "received",
      statusClass: null,
      observedAtMs: observedAtMs(),
      clientClosed: false,
    });

    let completed = false;
    responses.add(response);
    response.once("close", () => {
      responses.delete(response);
      if (!completed) {
        void record({
          routeLabel: label,
          requestKind: kind,
          scenario: control.scenario,
          phase: "closed",
          statusClass: null,
          observedAtMs: observedAtMs(),
          clientClosed: true,
        });
      }
    });
    const finish = (statusClass) => {
      completed = true;
      void record({
        routeLabel: label,
        requestKind: kind,
        scenario: control.scenario,
        phase: "completed",
        statusClass,
        observedAtMs: observedAtMs(),
        clientClosed: false,
      });
    };

    if (kind === "usage") {
      finish("2xx");
      sendJson(response, 200, { remaining: routeCounters.usage, unit: "QA" });
      return;
    }

    if (control.scenario === "http-500") {
      finish("5xx");
      sendJson(response, 500, {
        error: { code: "server_error", type: "server_error" },
      });
      return;
    }
    if (control.scenario === "http-429") {
      finish("429");
      sendJson(response, 429, {
        error: { code: "rate_limit_exceeded", type: "rate_limit_exceeded" },
      });
      return;
    }
    if (control.scenario === "account-error") {
      finish("account");
      sendJson(response, 403, {
        error: { code: "insufficient_quota", type: "insufficient_quota" },
      });
      return;
    }
    if (control.scenario === "connection-close") {
      finish("transport");
      request.socket.destroy();
      return;
    }
    if (control.scenario === "header-delay") {
      const timer = setTimeout(() => {
        timers.delete(timer);
        if (response.destroyed) return;
        finish("5xx");
        sendJson(response, 500, {
          error: { code: "server_error", type: "server_error" },
        });
      }, control.delayMs);
      timers.add(timer);
      return;
    }
    if (control.scenario === "success-json") {
      finish("2xx");
      sendJson(response, 200, {
        id: "fixture",
        status: "completed",
        output: [],
      });
      return;
    }

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    response.write('data: {"type":"response.created"}\n\n');

    if (control.scenario === "success-sse") {
      finish("2xx");
      response.end(`${meaningfulEvent()}${terminalEvent()}`);
      return;
    }
    if (control.scenario === "meaningful-delay") {
      const timer = setTimeout(() => {
        timers.delete(timer);
        if (response.destroyed) return;
        finish("2xx");
        response.end(`${meaningfulEvent()}${terminalEvent()}`);
      }, control.delayMs);
      timers.add(timer);
      return;
    }
    if (control.scenario === "meaningful-pending") {
      response.write(meaningfulEvent());
      return;
    }
    if (control.scenario === "meaningful-close") {
      response.write(meaningfulEvent());
      const timer = setTimeout(() => {
        timers.delete(timer);
        if (response.destroyed) return;
        finish("transport");
        response.destroy();
      }, control.delayMs);
      timers.add(timer);
      return;
    }
    if (control.scenario === "terminal-completed") {
      finish("2xx");
      response.end(terminalEvent());
      return;
    }
    if (control.scenario === "terminal-done") {
      finish("2xx");
      response.end("data: [DONE]\n\n");
      return;
    }
    if (control.scenario === "terminal-pending") {
      response.write(terminalEvent());
      return;
    }
    if (control.scenario === "lifecycle-only") {
      const interval = setInterval(
        () => {
          if (response.destroyed) {
            clearInterval(interval);
            timers.delete(interval);
            return;
          }
          response.write('data: {"type":"response.in_progress"}\n\n');
        },
        Math.max(control.delayMs, 1_000),
      );
      timers.add(interval);
    }
  };

  const handleController = async (request, response) => {
    const parsed = new URL(request.url ?? "/", "http://127.0.0.1");
    if (parsed.pathname === "/__control" && request.method === "POST") {
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        if (body.length > 8_192) {
          sendJson(response, 413, { error: "control_too_large" });
          return;
        }
      }
      let input;
      try {
        input = JSON.parse(body);
      } catch {
        sendJson(response, 400, { error: "invalid_control" });
        return;
      }
      if (input === null || Array.isArray(input) || typeof input !== "object") {
        sendJson(response, 400, { error: "invalid_control" });
        return;
      }
      if (input.nonce !== nonce) {
        sendJson(response, 403, { error: "invalid_control" });
        return;
      }
      if (input.action === "reset") {
        try {
          assertExactKeys(input, ["nonce", "action"], "fixture reset control");
        } catch {
          sendJson(response, 400, { error: "invalid_control" });
          return;
        }
        for (const label of ROUTE_LABELS) {
          counters.set(label, { responses: 0, usage: 0 });
        }
        unexpectedTrafficCount = 0;
        sendJson(response, 200, { ok: true });
        return;
      }
      try {
        assertExactKeys(
          input,
          ["nonce", "action", "routeLabel", "scenario", "delayMs"],
          "fixture set control",
        );
      } catch {
        sendJson(response, 400, { error: "invalid_control" });
        return;
      }
      if (
        input.action !== "set" ||
        !ROUTE_LABELS.includes(input.routeLabel) ||
        !ALLOWED_SCENARIOS.has(input.scenario) ||
        !Number.isInteger(input.delayMs) ||
        input.delayMs < 0 ||
        input.delayMs > 360_000
      ) {
        sendJson(response, 400, { error: "invalid_control" });
        return;
      }
      controls.set(input.routeLabel, {
        delayMs: input.delayMs,
        scenario: input.scenario,
      });
      sendJson(response, 200, { ok: true });
      return;
    }

    if (parsed.pathname === "/__snapshot" && request.method === "GET") {
      if (request.headers["x-qa-fixture-nonce"] !== nonce) {
        sendJson(response, 403, { error: "invalid_control" });
        return;
      }
      sendJson(response, 200, {
        unexpectedTrafficCount,
        routes: ROUTE_LABELS.map((routeLabel) => ({
          routeLabel,
          ...counters.get(routeLabel),
          ...controls.get(routeLabel),
        })),
      });
      return;
    }

    await rejectUnexpected(request, response);
  };

  try {
    const routes = [];
    for (const label of ROUTE_LABELS) {
      const server = safeServer((request, response) =>
        handleRoute(label, request, response),
      );
      const port = await listen(server);
      servers.push(server);
      routes.push({ label, baseUrl: `http://127.0.0.1:${port}/v1` });
    }
    const controller = safeServer(handleController);
    const controllerPort = await listen(controller);
    servers.push(controller);
    const controllerUrl = `http://127.0.0.1:${controllerPort}`;
    assertLoopbackUrl(controllerUrl, "fixture controller URL");
    const manifest = { schemaVersion: 1, nonce, controllerUrl, routes };
    await writeJsonAtomically(join(root, "fixture-manifest.json"), manifest);

    return {
      manifest,
      close() {
        closePromise ??= (async () => {
          for (const timer of timers) {
            clearTimeout(timer);
            clearInterval(timer);
          }
          timers.clear();
          for (const response of responses) response.destroy();
          await Promise.all(servers.map(closeServer));
          await ledgerWrite;
        })();
        return closePromise;
      },
    };
  } catch (error) {
    for (const response of responses) response.destroy();
    await Promise.allSettled(servers.map(closeServer));
    throw error;
  }
}

async function controlFixture(root, input) {
  const resolved = await resolveRunRoot(root);
  const manifest = await readJson(join(resolved.root, "fixture-manifest.json"));
  assertLoopbackUrl(manifest.controllerUrl, "fixture controller URL");
  const response = await fetch(`${manifest.controllerUrl}/__control`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ nonce: resolved.nonce, ...input }),
  });
  if (!response.ok) {
    throw new QaAcceptanceError(
      `Fixture control failed with ${response.status}.`,
    );
  }
}

async function snapshotFixture(root) {
  const resolved = await resolveRunRoot(root);
  const manifest = await readJson(join(resolved.root, "fixture-manifest.json"));
  assertLoopbackUrl(manifest.controllerUrl, "fixture controller URL");
  const response = await fetch(`${manifest.controllerUrl}/__snapshot`, {
    headers: { "x-qa-fixture-nonce": resolved.nonce },
  });
  if (!response.ok) {
    throw new QaAcceptanceError(
      `Fixture snapshot failed with ${response.status}.`,
    );
  }
  return response.json();
}

async function run() {
  const [command, ...arguments_] = process.argv.slice(2);
  const usage =
    "Usage: fixture <prepare|serve|reset|set|snapshot> [--root PATH]";
  if (command === "prepare") {
    const prepared = await createRunRoot();
    console.log(prepared.root);
    return;
  }
  if (!["serve", "reset", "set", "snapshot"].includes(command)) {
    throw new QaAcceptanceError(usage);
  }
  const root = optionValue(arguments_, "--root");
  if (command === "serve") {
    const fixture = await createFixtureServer({ root });
    console.log(`QA fixture listening on ${fixture.manifest.controllerUrl}`);
    const stop = async () => {
      await fixture.close();
      process.exitCode = 0;
    };
    process.once("SIGINT", stop);
    process.once("SIGTERM", stop);
    return new Promise(() => {});
  }
  if (command === "reset") {
    await controlFixture(root, { action: "reset" });
    return;
  }
  if (command === "set") {
    const delayOption = arguments_.includes("--delay-ms")
      ? Number(optionValue(arguments_, "--delay-ms"))
      : 0;
    await controlFixture(root, {
      action: "set",
      routeLabel: optionValue(arguments_, "--route"),
      scenario: optionValue(arguments_, "--scenario"),
      delayMs: delayOption,
    });
    return;
  }
  if (command === "snapshot") {
    console.log(JSON.stringify(await snapshotFixture(root), null, 2));
    return;
  }
  throw new QaAcceptanceError(usage);
}

if (process.argv[1] && process.argv[1] === fileURLToPath(import.meta.url)) {
  run().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}

export { controlFixture, snapshotFixture };
