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
  /**
   * A minimal one-family page: the given tier controls plus one pane per tier
   * that bill `$1.00` (standard) and `$2.00` (priority). Any outcome other than
   * a successful capture therefore means the matcher missed a control.
   */
  function tierPage(standardControl: string, priorityControl: string): string {
    return `<!doctype html>
<html lang="en">
  <body>
    <div class="content-switcher-root">
      <div role="radiogroup" aria-label="Service tier">
        ${standardControl}
        ${priorityControl}
      </div>
      <div class="content-pane">
        <table>
          <thead>
            <tr>
              <th scope="col">Model</th>
              <th scope="col">Input</th>
              <th scope="col">Cached input</th>
              <th scope="col">Output</th>
            </tr>
          </thead>
          <tbody>
            <tr><td>gpt-6-astra</td><td>$1.00</td><td>$0.10</td><td>$4.00</td></tr>
          </tbody>
        </table>
      </div>
      <div class="content-pane" hidden>
        <table>
          <thead>
            <tr>
              <th scope="col">Model</th>
              <th scope="col">Input</th>
              <th scope="col">Cached input</th>
              <th scope="col">Output</th>
            </tr>
          </thead>
          <tbody>
            <tr><td>gpt-6-astra</td><td>$2.00</td><td>$0.20</td><td>$8.00</td></tr>
          </tbody>
        </table>
      </div>
    </div>
  </body>
</html>`;
  }

  function option(attributes: string, label: string, checked = false): string {
    return `<button type="button" role="radio" ${attributes} aria-checked="${checked}">${label}</button>`;
  }

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
    // the hidden Ultrafast pane renders `$60.00`.
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
      captured.standard?.find((model) => model.id === "gpt-6.1-sol")?.long
        ?.input,
    ).toBe("$4.00");
  });

  it("reports a failure instead of guessing when a tier control is missing", async () => {
    const captured = await capture(
      // Dropping the priority control changes both its identity and its copy;
      // the family is then skipped and nothing is captured.
      pageFixture.replaceAll(
        'data-content-switcher-option="true" data-value="fast" aria-checked="false">Fast<',
        'data-content-switcher-option="true" data-value="turbo" aria-checked="false">Turbo<',
      ),
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
        '<div class="content-pane" data-content-switcher-pane="true" data-value="standard">\n            <table>',
        '<div class="content-pane" data-content-switcher-pane="true" data-value="standard">\n            <table hidden>',
      ),
    );

    expect(captured.ok).toBe(false);
    expect(captured.reason).toBe("pricing-table-not-found:standard");
  });

  describe("tier matcher", () => {
    const STANDARD_VALUE =
      'data-content-switcher-option="true" data-value="standard"';
    const PRIORITY_VALUE =
      'data-content-switcher-option="true" data-value="fast"';

    it("matches on `data-value` when the copy is `Fast`", async () => {
      const captured = await capture(
        tierPage(
          option(STANDARD_VALUE, "Standard", true),
          option(PRIORITY_VALUE, "Fast"),
        ),
      );

      expect(captured.ok).toBe(true);
      expect(captured.standard?.[0].short.input).toBe("$1.00");
      expect(captured.priority?.[0].short.input).toBe("$2.00");
    });

    it("prefers `data-value` over a copy that names the other tier", async () => {
      // Both radios render `Fast`; only `data-value` tells them apart.
      const captured = await capture(
        tierPage(
          option(STANDARD_VALUE, "Fast", true),
          option(PRIORITY_VALUE, "Fast"),
        ),
      );

      expect(captured.ok).toBe(true);
      expect(captured.standard?.[0].short.input).toBe("$1.00");
      expect(captured.priority?.[0].short.input).toBe("$2.00");
    });

    it("falls back to the 2026-09-29 `Fast mode` copy without `data-value`", async () => {
      const captured = await capture(
        tierPage(option("", "Standard", true), option("", "Fast mode")),
      );

      expect(captured.ok).toBe(true);
      expect(captured.standard?.[0].short.input).toBe("$1.00");
      expect(captured.priority?.[0].short.input).toBe("$2.00");
    });

    it("falls back to `aria-label` without `data-value`", async () => {
      const captured = await capture(
        tierPage(
          option('aria-label="Standard"', "Select a tier", true),
          option('aria-label="Fast"', "Select a tier"),
        ),
      );

      expect(captured.ok).toBe(true);
      expect(captured.standard?.[0].short.input).toBe("$1.00");
      expect(captured.priority?.[0].short.input).toBe("$2.00");
    });

    it("accepts the legacy `priority` `data-value`", async () => {
      const captured = await capture(
        tierPage(
          option(STANDARD_VALUE, "Standard", true),
          option(
            'data-content-switcher-option="true" data-value="priority"',
            "Priority",
          ),
        ),
      );

      expect(captured.ok).toBe(true);
      expect(captured.priority?.[0].short.input).toBe("$2.00");
    });

    it("reports `pricing-families-not-found` without a recognizable control", async () => {
      const captured = await capture(
        tierPage(
          option(
            'data-content-switcher-option="true" data-value="batch"',
            "Batch",
            true,
          ),
          option("", "Turbo"),
        ),
      );

      expect(captured.ok).toBe(false);
      expect(captured.reason).toBe("pricing-families-not-found");
      expect(captured.standard).toBeUndefined();
    });
  });
});
