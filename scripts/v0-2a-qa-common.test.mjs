import { mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  QA_ACCEPTANCE_MARKER_FILE,
  createRunRoot,
  resolveRunRoot,
} from "./v0-2a-qa-common.mjs";

const cleanupPaths = [];

afterEach(async () => {
  await Promise.all(
    cleanupPaths
      .splice(0)
      .map((path) => rm(path, { force: true, recursive: true })),
  );
});

async function temporaryParent() {
  const path = await mkdtemp(join(tmpdir(), "ai-router-qa-common-test-"));
  cleanupPaths.push(path);
  return path;
}

describe("V0.2A QA acceptance root", () => {
  it("resolves a marked nonce root below the selected temporary directory", async () => {
    const parent = await temporaryParent();
    const created = await createRunRoot(parent);

    await expect(resolveRunRoot(created.root, parent)).resolves.toEqual(
      created,
    );
  });

  it("rejects marker mismatches and symbolic-link roots", async () => {
    const parent = await temporaryParent();
    const created = await createRunRoot(parent);
    await writeFile(
      join(created.root, QA_ACCEPTANCE_MARKER_FILE),
      "different-nonce",
      "utf8",
    );
    await expect(resolveRunRoot(created.root, parent)).rejects.toThrow(
      "marker does not match",
    );

    const second = await createRunRoot(parent);
    const link = join(parent, "linked-root");
    await symlink(second.root, link);
    await expect(resolveRunRoot(link, parent)).rejects.toThrow(
      "must be a real directory",
    );
  });

  it("rejects roots outside the selected temporary directory", async () => {
    const selectedParent = await temporaryParent();
    const otherParent = await temporaryParent();
    const created = await createRunRoot(otherParent);

    await expect(resolveRunRoot(created.root, selectedParent)).rejects.toThrow(
      "outside the OS temporary directory",
    );
  });

  it("rejects control characters before filesystem access", async () => {
    await expect(resolveRunRoot(`/tmp/qa\u0000root`)).rejects.toThrow(
      "contains a control character",
    );
  });
});
