import { readFile, rm } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";

import { createRunRoot } from "./v0-2a-qa-common.mjs";
import {
  controlFixture,
  createFixtureServer,
  snapshotFixture,
} from "./v0-2a-qa-fixture.mjs";

const roots = [];

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
  );
});

async function fixture(options = {}) {
  const prepared = await createRunRoot();
  roots.push(prepared.root);
  const server = await createFixtureServer({
    root: prepared.root,
    ...options,
  });
  return { ...prepared, server };
}

function routeByLabel(running, label) {
  return running.server.manifest.routes.find((route) => route.label === label);
}

describe("V0.2A QA loopback fixture", () => {
  it("switches deterministic route scenarios and resets sanitized counters", async () => {
    const running = await fixture();
    try {
      const routeUrls = running.server.manifest.routes.map(
        (route) => new URL(route.baseUrl),
      );
      expect(new Set(routeUrls.map((url) => url.port)).size).toBe(4);
      expect(routeUrls.map((url) => url.port)).not.toContain(
        new URL(running.server.manifest.controllerUrl).port,
      );
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "A",
        scenario: "http-429",
        delayMs: 0,
      });
      const routeA = running.server.manifest.routes.find(
        (route) => route.label === "A",
      );
      const response = await fetch(`${routeA.baseUrl}/responses`, {
        method: "POST",
        headers: { authorization: "Bearer must-not-be-recorded" },
        body: "must-not-be-recorded",
      });
      expect(response.status).toBe(429);
      const attempted = await snapshotFixture(running.root);
      expect(attempted.routes[0]).toMatchObject({
        routeLabel: "A",
        responses: 1,
        scenario: "http-429",
      });
      await controlFixture(running.root, { action: "reset" });
      const reset = await snapshotFixture(running.root);
      expect(reset.routes[0]).toMatchObject({ routeLabel: "A", responses: 0 });
      const ledger = await readFile(
        `${running.root}/fixture-events.sanitized.jsonl`,
        "utf8",
      );
      expect(ledger).not.toContain("must-not-be-recorded");
      expect(ledger).not.toContain("authorization");
      const events = ledger
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      expect(events.map((event) => event.observedAtMs)).toEqual(
        [...events]
          .map((event) => event.observedAtMs)
          .sort((left, right) => left - right),
      );
    } finally {
      await running.server.close();
    }
  });

  it("records downstream closure without storing stream bytes", async () => {
    const running = await fixture();
    try {
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "B",
        scenario: "pending",
        delayMs: 0,
      });
      const controller = new AbortController();
      const routeB = running.server.manifest.routes.find(
        (route) => route.label === "B",
      );
      const response = await fetch(`${routeB.baseUrl}/responses`, {
        method: "POST",
        body: "stream-body-must-not-be-recorded",
        signal: controller.signal,
      });
      expect(response.status).toBe(200);
      controller.abort();
      await new Promise((resolve) => setTimeout(resolve, 25));
      const ledger = await readFile(
        `${running.root}/fixture-events.sanitized.jsonl`,
        "utf8",
      );
      expect(ledger).toContain('"clientClosed":true');
      expect(ledger).not.toContain("stream-body-must-not-be-recorded");
    } finally {
      await running.server.close();
    }
  });

  it("supports committed pending and post-commit transport failure streams", async () => {
    const running = await fixture();
    try {
      const routeC = running.server.manifest.routes.find(
        (route) => route.label === "C",
      );
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "C",
        scenario: "meaningful-pending",
        delayMs: 0,
      });
      const controller = new AbortController();
      const pending = await fetch(`${routeC.baseUrl}/responses`, {
        method: "POST",
        signal: controller.signal,
      });
      const reader = pending.body.getReader();
      const first = await reader.read();
      expect(new TextDecoder().decode(first.value)).toContain(
        "response.output_text.delta",
      );
      controller.abort();

      await controlFixture(running.root, {
        action: "set",
        routeLabel: "C",
        scenario: "meaningful-close",
        delayMs: 25,
      });
      const closed = await fetch(`${routeC.baseUrl}/responses`, {
        method: "POST",
      });
      await expect(closed.text()).rejects.toThrow();
      await new Promise((resolve) => setTimeout(resolve, 25));
      const ledger = await readFile(
        `${running.root}/fixture-events.sanitized.jsonl`,
        "utf8",
      );
      expect(ledger).toContain('"scenario":"meaningful-pending"');
      expect(ledger).toContain(
        '"scenario":"meaningful-close","phase":"completed","statusClass":"transport"',
      );
    } finally {
      await running.server.close();
    }
  });

  it("rejects non-allowlisted scenario controls", async () => {
    const running = await fixture();
    try {
      await expect(
        controlFixture(running.root, {
          action: "set",
          routeLabel: "A",
          scenario: "arbitrary-response",
          delayMs: 0,
        }),
      ).rejects.toThrow("Fixture control failed with 400");
    } finally {
      await running.server.close();
    }
  });

  it("surfaces unexpected traffic as a scenario-blocking counter", async () => {
    const running = await fixture();
    try {
      const routeC = running.server.manifest.routes.find(
        (route) => route.label === "C",
      );
      const response = await fetch(`${routeC.baseUrl}/not-allowlisted`);
      expect(response.status).toBe(404);
      await expect(snapshotFixture(running.root)).resolves.toMatchObject({
        unexpectedTrafficCount: 1,
      });
    } finally {
      await running.server.close();
    }
  });

  it("advertises the manifest protocol for every route", async () => {
    const running = await fixture();
    try {
      expect(
        Object.fromEntries(
          running.server.manifest.routes.map((route) => [
            route.label,
            route.protocol,
          ]),
        ),
      ).toEqual({
        A: "responses",
        B: "chat_completions",
        C: "responses",
        D: "responses",
      });
    } finally {
      await running.server.close();
    }
  });

  it("rejects an unknown route protocol before serving", async () => {
    await expect(
      fixture({ protocols: { B: "anthropic_messages" } }),
    ).rejects.toThrow("unsupported protocol");
  });

  it("counts a chat route request on /v1/chat/completions", async () => {
    const running = await fixture();
    try {
      const routeB = routeByLabel(running, "B");
      const response = await fetch(`${routeB.baseUrl}/chat/completions`, {
        method: "POST",
        body: "must-not-be-recorded",
      });
      expect(response.status).toBe(200);
      const snapshot = await snapshotFixture(running.root);
      expect(snapshot.routes[1]).toMatchObject({
        routeLabel: "B",
        protocol: "chat_completions",
        responses: 0,
        chat: 1,
        usage: 0,
      });
      await controlFixture(running.root, { action: "reset" });
      const reset = await snapshotFixture(running.root);
      expect(reset.routes[1]).toMatchObject({
        routeLabel: "B",
        responses: 0,
        chat: 0,
        usage: 0,
      });
    } finally {
      await running.server.close();
    }
  });

  it("answers a chat route JSON request with a Chat completion body", async () => {
    const running = await fixture();
    try {
      const routeB = routeByLabel(running, "B");
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "B",
        scenario: "success-json",
        delayMs: 0,
      });
      const response = await fetch(`${routeB.baseUrl}/chat/completions`, {
        method: "POST",
        body: "must-not-be-recorded",
      });
      expect(response.headers.get("content-type")).toContain(
        "application/json",
      );
      await expect(response.json()).resolves.toEqual({
        id: "chatcmpl-fixture",
        model: "fixture",
        choices: [
          {
            index: 0,
            message: { role: "assistant", content: "fixture" },
            finish_reason: "stop",
          },
        ],
        usage: {
          prompt_tokens: 11,
          completion_tokens: 5,
          total_tokens: 16,
          prompt_tokens_details: { cached_tokens: 2 },
        },
      });
    } finally {
      await running.server.close();
    }
  });

  it("answers a chat route SSE request with Chat delta frames", async () => {
    const running = await fixture();
    try {
      const routeB = routeByLabel(running, "B");
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "B",
        scenario: "success-sse",
        delayMs: 0,
      });
      const response = await fetch(`${routeB.baseUrl}/chat/completions`, {
        method: "POST",
        body: "must-not-be-recorded",
      });
      expect(response.headers.get("content-type")).toContain(
        "text/event-stream",
      );
      const body = await response.text();
      expect(body).toContain('"object":"chat.completion.chunk"');
      expect(body).toContain('"delta":{"content":"fixture"}');
      expect(body).toContain('"name":"tool_search"');
      expect(body).toContain('"usage":{"prompt_tokens":11');
      expect(body).toContain('"finish_reason":"stop"');
      expect(body.trimEnd().endsWith("data: [DONE]")).toBe(true);
      expect(body).not.toContain("response.completed");
      expect(body).not.toContain("response.output_text.delta");
    } finally {
      await running.server.close();
    }
  });

  it("keeps a Responses route answering with Responses SSE", async () => {
    const running = await fixture();
    try {
      const routeA = routeByLabel(running, "A");
      await controlFixture(running.root, {
        action: "set",
        routeLabel: "A",
        scenario: "success-sse",
        delayMs: 0,
      });
      const response = await fetch(`${routeA.baseUrl}/responses`, {
        method: "POST",
        body: "must-not-be-recorded",
      });
      const body = await response.text();
      expect(body).toContain("response.output_text.delta");
      expect(body).toContain("response.completed");
      expect(body).not.toContain("chat.completion.chunk");
      await expect(snapshotFixture(running.root)).resolves.toMatchObject({
        routes: [
          { routeLabel: "A", responses: 1, chat: 0, usage: 0 },
          { routeLabel: "B", responses: 0, chat: 0, usage: 0 },
          { routeLabel: "C", responses: 0, chat: 0, usage: 0 },
          { routeLabel: "D", responses: 0, chat: 0, usage: 0 },
        ],
      });
    } finally {
      await running.server.close();
    }
  });

  it("records only bounded chat ledger fields without bodies or headers", async () => {
    const running = await fixture();
    try {
      const routeB = routeByLabel(running, "B");
      const response = await fetch(`${routeB.baseUrl}/chat/completions`, {
        method: "POST",
        headers: { authorization: "Bearer chat-must-not-be-recorded" },
        body: "chat-body-must-not-be-recorded",
      });
      await response.text();
      const ledger = await readFile(
        `${running.root}/fixture-events.sanitized.jsonl`,
        "utf8",
      );
      expect(ledger).toContain('"requestKind":"chat"');
      expect(ledger).not.toContain("chat-body-must-not-be-recorded");
      expect(ledger).not.toContain("chat-must-not-be-recorded");
      expect(ledger).not.toContain("authorization");
      expect(ledger).not.toContain("/chat/completions");
      const kinds = new Set(
        ledger
          .trim()
          .split("\n")
          .map((line) => JSON.parse(line).requestKind),
      );
      expect([...kinds]).toEqual(["chat"]);
    } finally {
      await running.server.close();
    }
  });
});
