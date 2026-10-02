import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("WebView entrypoint CSP support", () => {
  it.each(["menu.html", "settings.html"])(
    "%s provides one inert style nonce sentinel for runtime styles",
    (file) => {
      const document = new DOMParser().parseFromString(
        readFileSync(resolve(process.cwd(), file), "utf8"),
        "text/html",
      );
      const sentinels = document.querySelectorAll("#app-csp-style-nonce");
      expect(sentinels).toHaveLength(1);
      expect(sentinels[0].tagName).toBe("STYLE");
      expect(sentinels[0].textContent).toBe("");
    },
  );
});
