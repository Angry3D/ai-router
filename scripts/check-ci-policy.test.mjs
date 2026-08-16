import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import {
  checkCiPolicy,
  validateActionPins,
  validateCiWorkflow,
  validateDependabotConfig,
  validateSecurityWorkflow,
  validateWorkflowSafety,
} from "./check-ci-policy.mjs";

const projectRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

async function readDependabotConfig() {
  const { parse } = await import("yaml");
  return parse(
    await readFile(join(projectRoot, ".github/dependabot.yml"), "utf8"),
  );
}

describe("GitHub CI policy", () => {
  it("accepts the repository workflows and Dependabot configuration", async () => {
    await expect(checkCiPolicy(projectRoot)).resolves.toMatchObject({
      dependabotEcosystems: 3,
      requiredChecks: [
        "Required / Node quality",
        "Required / Rust quality",
        "Required / Generated and contracts",
        "Required / Protocol compatibility",
        "Required / Repository policy",
        "Security / Dependency review",
        "Security / CodeQL",
      ],
      workflows: 3,
    });
  });

  it("rejects mutable or uncommented action references", () => {
    expect(() =>
      validateActionPins(
        "fixture.yml",
        "steps:\n  - uses: actions/checkout@v4\n",
      ),
    ).toThrow("full SHA and version comment");
    expect(() =>
      validateActionPins(
        "fixture.yml",
        "steps:\n  - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd\n",
      ),
    ).toThrow("full SHA and version comment");
    expect(() =>
      validateActionPins(
        "fixture.yml",
        "steps:\n  - uses: actions/checkout@0000000000000000000000000000000000000000 # v6.0.2\n",
      ),
    ).toThrow("reviewed SHA/version pair");
    expect(() =>
      validateActionPins(
        "fixture.yml",
        [
          "steps:",
          "  - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2",
          "  - uses: docker://alpine:latest",
          "",
        ].join("\n"),
      ),
    ).toThrow("unsupported or mutable action reference");
    expect(() =>
      validateActionPins(
        "fixture.yml",
        [
          "steps:",
          "  - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v5.0.0",
          "  - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2",
          "",
        ].join("\n"),
      ),
    ).toThrow("reviewed SHA/version pair");
  });

  it("rejects expanded permissions, secrets, artifacts, and lifecycle commands", () => {
    const checkoutStep = {
      uses: "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd",
      with: { "persist-credentials": false },
    };
    const base = {
      concurrency: { "cancel-in-progress": true, group: "fixture" },
      jobs: { fixture: { steps: [checkoutStep], "timeout-minutes": 5 } },
      permissions: { contents: "write" },
    };
    expect(() =>
      validateWorkflowSafety(
        "fixture.yml",
        "uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n",
        base,
      ),
    ).toThrow("contents: read");

    const safeObject = { ...base, permissions: { contents: "read" } };
    for (const forbidden of [
      `run: echo ${"${{"} secrets.SIGNING_KEY }}`,
      `run: echo ${"${{"} secrets['SIGNING_KEY'] }}`,
      "uses: actions/upload-artifact@0000000000000000000000000000000000000000 # v4",
      "run: open '/Applications/AI Router.app'",
      "run: cat '$HOME/Library/Application Support/com.relax.airouter/ai-router.db'",
      "run: cat '$HOME/.codex/config.toml'",
    ]) {
      expect(() =>
        validateWorkflowSafety(
          "fixture.yml",
          `${forbidden}\nuses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n`,
          safeObject,
        ),
      ).toThrow("forbidden");
    }
  });

  it("requires hardened checkout in every job and reserves security-events write for CodeQL", () => {
    const content =
      "uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n";
    const workflow = {
      concurrency: { "cancel-in-progress": true, group: "fixture" },
      jobs: {
        fixture: {
          steps: [],
          "timeout-minutes": 5,
        },
      },
      permissions: { contents: "read" },
    };
    expect(() =>
      validateWorkflowSafety("fixture.yml", content, workflow),
    ).toThrow("exactly one immutable checkout");

    workflow.jobs.fixture.steps = [
      {
        uses: "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd",
      },
    ];
    expect(() =>
      validateWorkflowSafety("fixture.yml", content, workflow),
    ).toThrow("persist-credentials: false");

    workflow.jobs.fixture.steps[0].with = { "persist-credentials": false };
    workflow.jobs.fixture.permissions = {
      contents: "read",
      "security-events": "write",
    };
    expect(() =>
      validateWorkflowSafety("fixture.yml", content, workflow),
    ).toThrow("only CodeQL");
  });

  it("rejects required jobs that can skip or omit their local commands", async () => {
    const { parse } = await import("yaml");
    const source = await readFile(
      join(projectRoot, ".github/workflows/ci.yml"),
      "utf8",
    );
    const conditional = parse(source);
    conditional.jobs["node-quality"].if = "false";
    expect(() => validateCiWorkflow(conditional)).toThrow(
      "must not be conditional",
    );

    const missingCommand = parse(source);
    missingCommand.jobs["node-quality"].steps = missingCommand.jobs[
      "node-quality"
    ].steps.filter((step) => step.run !== "pnpm lint");
    expect(() => validateCiWorkflow(missingCommand)).toThrow(
      "must run exactly once: pnpm lint",
    );
  });

  it("rejects security jobs without their owning analysis action", async () => {
    const { parse } = await import("yaml");
    const source = await readFile(
      join(projectRoot, ".github/workflows/security.yml"),
      "utf8",
    );
    const workflow = parse(source);
    workflow.jobs.codeql.steps = workflow.jobs.codeql.steps.filter(
      (step) => !step.uses?.startsWith("github/codeql-action/analyze@"),
    );
    expect(() => validateSecurityWorkflow(workflow)).toThrow(
      "must use exactly one github/codeql-action/analyze action",
    );
  });

  it("rejects incomplete Dependabot ecosystem coverage", async () => {
    const config = await readDependabotConfig();
    config.updates = config.updates.filter(
      (update) => update["package-ecosystem"] !== "cargo",
    );
    expect(() => validateDependabotConfig(config)).toThrow("cargo");
  });

  it("rejects unsafe Dependabot version-update policies", async () => {
    const missingAllow = await readDependabotConfig();
    delete missingAllow.updates[0].allow;
    expect(() => validateDependabotConfig(missingAllow)).toThrow(
      "allow only ordinary minor/patch updates",
    );

    const allowsMajor = await readDependabotConfig();
    allowsMajor.updates[1].allow[0]["update-types"].push(
      "version-update:semver-major",
    );
    expect(() => validateDependabotConfig(allowsMajor)).toThrow(
      "allow only ordinary minor/patch updates",
    );

    const ignoresMajor = await readDependabotConfig();
    ignoresMajor.updates[2].ignore = [
      {
        "dependency-name": "*",
        "update-types": ["version-update:semver-major"],
      },
    ];
    expect(() => validateDependabotConfig(ignoresMajor)).toThrow(
      "avoid ignore rules",
    );

    const ignoresAllUpdates = await readDependabotConfig();
    ignoresAllUpdates.updates[0].ignore = [{ "dependency-name": "*" }];
    expect(() => validateDependabotConfig(ignoresAllUpdates)).toThrow(
      "avoid ignore rules",
    );

    const expandedQueue = await readDependabotConfig();
    expandedQueue.updates[0]["open-pull-requests-limit"] = 3;
    expect(() => validateDependabotConfig(expandedQueue)).toThrow(
      "cap ordinary PRs at 2",
    );
  });
});
