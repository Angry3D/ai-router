/*
 * Pricing extraction script for the hidden synchronization WebView.
 *
 * Injected by `src-tauri/src/pricing_sync.rs` as the webview initialization
 * script, so it runs at document start on
 * https://developers.openai.com/api/docs/pricing/ without using any Tauri API
 * (the window matches no capability, so every command is denied to that remote
 * origin): it reads the rendered tables, switches the segmented service-tier
 * controls, and reports one JSON document by navigating to
 * `airouter-pricing-capture://v1/<base64url payload>`. Rust cancels that
 * navigation and parses the payload.
 *
 * Contract (verified against the live page in the C4 harness on 2026-09-29):
 * - only the families that render both a `Standard` and a `Fast mode`
 *   `button[role="radio"]` are synchronized; `Fast mode` is the renamed
 *   Priority tier (`service_tier: "priority"`). Families without both controls
 *   (image and other non-GPT families) are skipped;
 * - only tables that are actually visible are read: an unselected pane carries
 *   `hidden` and keeps its rows in the DOM;
 * - a table's column layout comes from its header, never from a fixed offset:
 *   the model column is the header cell labelled `Model`, columns before it are
 *   row scoping (such as `Category`) and are ignored, and every column after it
 *   must be a recognized price column: `Input`, `Cached input`,
 *   `Cache writes`, `Output`. A table may omit `Cache writes` (which then means
 *   "not offered") and may omit the long context columns entirely;
 * - the service-tier tables of a family render the short and the long context
 *   band side by side under a header group such as `Short context` /
 *   `Long context`; the group names decide which columns belong to which band.
 *   A model whose long-context `Input` cell renders `-` has no long band;
 * - the page is not required to render the 272K boundary: the band boundary is
 *   fixed at 272,000/272,001 tokens, and Rust only fails the capture when the
 *   page confirms a *different* threshold;
 * - a missing control, table, column, or cell fails the capture with a reason
 *   instead of reporting partial prices.
 *
 * The payload is validated again in Rust (`router_core::pricing_capture`).
 */
/* global document, location, window */
(function () {
  "use strict";

  const CAPTURE_SCHEME = "airouter-pricing-capture";
  const CAPTURE_VERSION = "v1";
  const MAX_PAYLOAD_BYTES = 262144;
  const POLL_INTERVAL_MS = 200;
  const FAMILY_TIMEOUT_MS = 15000;
  const SWITCH_TIMEOUT_MS = 3000;
  const FAMILY_SELECTOR = ".content-switcher-root";
  const RADIO_SELECTOR = 'button[role="radio"]';
  const SELECTED_ATTRIBUTES = [
    ["aria-checked", "true"],
    ["aria-selected", "true"],
    ["data-state", "checked"],
  ];
  const TIER_LABELS = { standard: "Standard", priority: "Fast mode" };
  const TIER_ORDER = ["standard", "priority"];
  const MODEL_PATTERN = /^model$/i;
  const LONG_BAND_PATTERN = /long(?:\s*|-)?context|^long$/i;
  const COLUMN_PATTERNS = [
    ["cachedInput", /cached\s*input/i],
    ["cacheWrite", /cache\s*write/i],
    ["output", /output/i],
    ["input", /input/i],
  ];
  const REQUIRED_COLUMN_KEYS = ["input", "cachedInput", "output"];
  const THRESHOLD_PATTERN = /\d+(?:[.,]\d+)?\s*K\b/gi;
  const ABSENT = "-";

  let reported = false;

  function normalize(value) {
    return String(value === null || value === undefined ? "" : value)
      .replace(/\s+/g, " ")
      .trim();
  }

  function text(element) {
    return normalize(element === null ? "" : element.textContent);
  }

  function firstToken(value) {
    return normalize(value).split(" ")[0];
  }

  function base64Url(value) {
    const bytes = new TextEncoder().encode(value);
    let binary = "";
    for (const byte of bytes) {
      binary += String.fromCharCode(byte);
    }
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function send(payload) {
    if (reported) return;
    reported = true;
    let encoded = base64Url(JSON.stringify(payload));
    if (encoded.length > MAX_PAYLOAD_BYTES) {
      encoded = base64Url(JSON.stringify({ ok: false, reason: "payload-too-large" }));
    }
    location.href = `${CAPTURE_SCHEME}://${CAPTURE_VERSION}/${encoded}`;
  }

  function fail(reason) {
    send({ ok: false, reason });
  }

  function waitFor(condition, timeoutMs) {
    return new Promise((resolve) => {
      const until = Date.now() + timeoutMs;
      const step = () => {
        let value = null;
        try {
          value = condition();
        } catch {
          value = null;
        }
        if (value) {
          resolve(value);
          return;
        }
        if (Date.now() >= until) {
          resolve(null);
          return;
        }
        window.setTimeout(step, POLL_INTERVAL_MS);
      };
      step();
    });
  }

  function isSelected(radio) {
    return SELECTED_ATTRIBUTES.some(
      ([attribute, value]) => radio.getAttribute(attribute) === value,
    );
  }

  function isVisible(element, root) {
    let current = element;
    while (current && current.nodeType === 1) {
      if (current.hasAttribute("hidden")) return false;
      if (current.getAttribute("aria-hidden") === "true") return false;
      if (current === root) return true;
      current = current.parentElement;
    }
    return true;
  }

  function visibleTables(root) {
    return Array.from(root.querySelectorAll("table")).filter((table) =>
      isVisible(table, root),
    );
  }

  function paneSignature(root) {
    return visibleTables(root)
      .map((table) => {
        const row = table.querySelector("tbody tr");
        return row && row.children.length > 0 ? text(row.children[0]) : "";
      })
      .join("|");
  }

  function findRadio(root, label) {
    const radios = Array.from(root.querySelectorAll(RADIO_SELECTOR));
    return (
      radios.find(
        (radio) => text(radio) === label || normalize(radio.getAttribute("aria-label")) === label,
      ) || null
    );
  }

  /** One header cell label per grid column, expanding `colspan`. */
  function expandRow(row) {
    const labels = [];
    for (const cell of Array.from(row.children)) {
      const colspan = Math.max(1, Number(cell.getAttribute("colspan")) || 1);
      for (let span = 0; span < colspan; span += 1) labels.push(text(cell));
    }
    return labels;
  }

  function matchColumn(label) {
    const match = COLUMN_PATTERNS.find(([, pattern]) => pattern.test(label));
    return match ? match[0] : null;
  }

  function band(values) {
    const recorded = {};
    for (const { key, value } of values) {
      if (key === null || Object.hasOwn(recorded, key)) {
        throw new Error("pricing-columns-unknown");
      }
      recorded[key] = value;
    }
    for (const key of REQUIRED_COLUMN_KEYS) {
      if (!Object.hasOwn(recorded, key)) {
        throw new Error("pricing-columns-unknown");
      }
    }
    return recorded;
  }

  function readTable(table) {
    const headerRows = Array.from(table.querySelectorAll("thead tr"));
    if (headerRows.length === 0) throw new Error("pricing-header-not-found");
    const labels = expandRow(headerRows[headerRows.length - 1]);
    const groupLabels = headerRows.slice(0, -1).flatMap(expandRow);
    // The model column is named by the header; a header that keeps `Model` in an
    // earlier row leaves the label row with price columns only.
    const modelIndex = labels.findIndex((label) => MODEL_PATTERN.test(label));
    const offset = modelIndex >= 0 ? 0 : 1;
    const columns = labels.map(matchColumn);
    for (let index = 0; index < columns.length; index += 1) {
      if (index > modelIndex && columns[index] === null) {
        throw new Error("pricing-columns-unknown");
      }
    }
    const longStart = groupLabels.findIndex((label) => LONG_BAND_PATTERN.test(label));
    const thresholds = (groupLabels.join(" ") + " " + labels.join(" ")).match(
      THRESHOLD_PATTERN,
    ) || [];
    const models = [];
    for (const row of Array.from(table.querySelectorAll("tbody tr"))) {
      const cells = Array.from(row.children);
      if (cells.length !== labels.length + offset) {
        throw new Error("pricing-columns-mismatch");
      }
      const id = firstToken(text(modelIndex >= 0 ? cells[modelIndex] : cells[0]));
      if (id === "") throw new Error("pricing-model-missing");
      const short = [];
      const long = [];
      for (let index = 0; index < columns.length; index += 1) {
        if (columns[index] === null) continue;
        const value = text(cells[index + offset]);
        const entry = { key: columns[index], value };
        if (longStart >= 0 && index >= longStart) long.push(entry);
        else short.push(entry);
      }
      const model = { id, short: band(short) };
      if (long.length > 0) {
        const longInput = long.find(({ key }) => key === "input").value;
        if (longInput !== ABSENT && longInput !== "") model.long = band(long);
      }
      models.push(model);
    }
    if (models.length === 0) throw new Error("pricing-rows-not-found");
    return {
      models,
      thresholds: thresholds.map((token) => token.replace(/\s+/g, "")),
    };
  }

  async function collect() {
    const tiers = { standard: [], priority: [] };
    const thresholds = new Set();
    let families = 0;
    for (const root of Array.from(document.querySelectorAll(FAMILY_SELECTOR))) {
      const controls = {
        standard: findRadio(root, TIER_LABELS.standard),
        priority: findRadio(root, TIER_LABELS.priority),
      };
      if (!controls.standard || !controls.priority) continue;
      families += 1;
      for (const tier of TIER_ORDER) {
        const radio = controls[tier];
        if (!isSelected(radio)) {
          const before = paneSignature(root);
          radio.click();
          const switched = await waitFor(
            () => isSelected(radio) || paneSignature(root) !== before,
            SWITCH_TIMEOUT_MS,
          );
          if (!switched) throw new Error(`tier-switch-failed:${tier}`);
        }
        const tables = visibleTables(root);
        if (tables.length === 0) throw new Error(`pricing-table-not-found:${tier}`);
        for (const table of tables) {
          const parsed = readTable(table);
          for (const token of parsed.thresholds) thresholds.add(token);
          for (const model of parsed.models) {
            if (tiers[tier].some((existing) => existing.id === model.id)) {
              throw new Error(`duplicate-model:${model.id}`);
            }
            tiers[tier].push(model);
          }
        }
      }
    }
    if (families === 0) throw new Error("pricing-families-not-found");
    return {
      ok: true,
      thresholds: Array.from(thresholds).sort(),
      standard: tiers.standard,
      priority: tiers.priority,
    };
  }

  async function run() {
    const ready = await waitFor(
      () => document.querySelectorAll(FAMILY_SELECTOR).length > 0,
      FAMILY_TIMEOUT_MS,
    );
    if (!ready) {
      fail("pricing-families-not-found");
      return;
    }
    try {
      send(await collect());
    } catch (error) {
      fail(String(error && error.message ? error.message : error));
    }
  }

  void run();
})();
