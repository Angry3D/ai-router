import { describe, expect, it } from "vitest";

import { formatBalanceScript } from "./formatBalanceScript";

describe("balance script formatting", () => {
  it("formats JavaScript expressions without evaluating them", async () => {
    await expect(
      formatBalanceScript("(()=>({request:{method:'GET'},extractor:(x)=>x}))()"),
    ).resolves.toContain('method: "GET"');
  });

  it("rejects invalid JavaScript", async () => {
    await expect(formatBalanceScript("(() => ({")).rejects.toThrow();
  });
});
