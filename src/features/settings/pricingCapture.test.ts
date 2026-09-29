import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import pageFixture from "../../../fixtures/pricing-capture-page.html?raw";
import payloadFixture from "../../../fixtures/pricing-capture-payload.json";
import captureScript from "../../../fixtures/pricing-capture-script.js?raw";

const CAPTURE_PREFIX = "airouter-pricing-capture://v1/";

interface CapturedBand {
  input: string;
  cachedInput: string;
  cacheWrite: string;
  output: string;
}

interface CapturedModel {
  id: string;
  short: CapturedBand;
  long?: CapturedBand;
}

interface CapturePayload {
  ok: boolean;
  reason?: string;
  thresholds?: string[];
  standard?: CapturedModel[];
  priority?: CapturedModel[];
}

/**
 * Models the React behavior of the pricing page: the segmented control of a
 * family selects the pane with the same index, hides the others, and marks only
 * the clicked radio as checked.
 */
function wireSegmentedControls(document: Document) {
  for (const root of Array.from(
    document.querySelectorAll(".content-switcher-root"),
  )) {
    const radios = Array.from(root.querySelectorAll('button[role="radio"]'));
    const panes = Array.from(root.querySelectorAll(".content-pane"));
    radios.forEach((radio, index) => {
      radio.addEventListener("click", () => {
        radios.forEach((candidate) =>
          candidate.setAttribute("aria-checked", String(candidate === radio)),
        );
        panes.forEach((pane, paneIndex) => {
          if (paneIndex === index) pane.removeAttribute("hidden");
          else pane.setAttribute("hidden", "");
        });
      });
    });
  }
}

function decodeBase64Url(value: string): string {
  const base64 = value
    .replace(/-/g, "+")
    .replace(/_/g, "/")
    .padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(base64);
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

async function capture(pageHtml: string): Promise<CapturePayload> {
  const document = new DOMParser().parseFromString(pageHtml, "text/html");
  wireSegmentedControls(document);
  const location = { href: "" };
  const run = new Function(
    "window",
    "document",
    "location",
    captureScript,
  ) as (scope: Window, document: Document, location: { href: string }) => void;
  run(window, document, location);

  await waitFor(() => expect(location.href).not.toBe(""));
  expect(location.href.startsWith(CAPTURE_PREFIX)).toBe(true);
  return JSON.parse(
    decodeBase64Url(location.href.slice(CAPTURE_PREFIX.length)),
  ) as CapturePayload;
}

describe("pricing capture script", () => {
  it("extracts both synced tiers from the visible panes of the fixture page", async () => {
    const captured = await capture(pageFixture);

    // The payload is the cross-language contract: `router_core::pricing_capture`
    // parses exactly this document.
    expect(captured).toEqual(payloadFixture);
    expect(captured.ok).toBe(true);
    // The live page renders no threshold text; the boundary is authored in Rust.
    expect(captured.thresholds).toEqual([]);
  });

  it("reads the pane the segmented control selected, never the hidden ones", async () => {
    const captured = await capture(pageFixture);

    const standard = captured.standard?.find(
      (model) => model.id === "gpt-6-astra",
    );
    // `$10.00` is the Standard pane; the hidden Batch pane renders `$5.00` and
    // the hidden Flex pane renders `$12.00`.
    expect(standard?.short.input).toBe("$10.00");
    expect(
      captured.priority?.find((model) => model.id === "gpt-6-astra")?.short
        .input,
    ).toBe("$20.00");
  });

  it("reads a table that renders neither a cache-write column nor a long band", async () => {
    const captured = await capture(pageFixture);

    const specialized = captured.standard?.find(
      (model) => model.id === "gpt-5.3-codex",
    );
    expect(specialized?.short).toEqual({
      input: "$1.75",
      cachedInput: "$0.175",
      output: "$14.00",
    });
    expect(specialized).not.toHaveProperty("long");
    // The flagship table still reports both bands and the cache-write rate.
    const astra = captured.standard?.find((model) => model.id === "gpt-6-astra");
    expect(astra?.short).toEqual({
      input: "$10.00",
      cachedInput: "$1.00",
      cacheWrite: "$12.50",
      output: "$50.00",
    });
    expect(astra?.long?.input).toBe("$20.00");
  });

  it("omits the long band when the page renders a dash for its input", async () => {
    const captured = await capture(
      // The flagship Standard pane renders `$20.00` for the long-context input
      // of `gpt-6-astra`; a dash means the model has no long band.
      pageFixture.replace("<td>$20.00</td>", "<td>-</td>"),
    );

    const astra = captured.standard?.find((model) => model.id === "gpt-6-astra");
    expect(astra).not.toHaveProperty("long");
    expect(astra?.short.input).toBe("$10.00");
    // Other models in the same table keep their long band.
    expect(
      captured.standard?.find((model) => model.id === "gpt-6-sol")?.long?.input,
    ).toBe("$4.00");
  });

  it("reports a failure instead of guessing when a tier control is missing", async () => {
    const captured = await capture(
      pageFixture.replaceAll("Fast mode", "Priority"),
    );

    expect(captured.ok).toBe(false);
    expect(captured.reason).toBe("pricing-families-not-found");
    expect(captured.standard).toBeUndefined();
  });

  it("reports a failure when a price column is unrecognized", async () => {
    // The first occurrence belongs to the selected flagship Standard pane.
    const captured = await capture(
      pageFixture.replace(
        '<th scope="col">Cache writes</th>',
        '<th scope="col">Cache creation</th>',
      ),
    );

    expect(captured.ok).toBe(false);
    expect(captured.reason).toBe("pricing-columns-unknown");
  });

  it("reports a failure when a model is billed twice in the same tier", async () => {
    const captured = await capture(
      pageFixture.replace(
        "<td>gpt-rosalind-research</td>",
        "<td>gpt-5.3-codex</td>",
      ),
    );

    expect(captured.ok).toBe(false);
    expect(captured.reason).toBe("duplicate-model:gpt-5.3-codex");
  });

  it("reports a failure when the selected pane renders no visible table", async () => {
    const captured = await capture(
      pageFixture.replace(
        '<div class="content-pane">\n            <table>',
        '<div class="content-pane">\n            <table hidden>',
      ),
    );

    expect(captured.ok).toBe(false);
    expect(captured.reason).toBe("pricing-table-not-found:standard");
  });
});
