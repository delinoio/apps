// SPDX-License-Identifier: Apache-2.0
// Browser-only API fixture. No production source imports this module.
export function installBrowserFixture() {
  const original = window.fetch;
  const commits = ["c".repeat(40), "b".repeat(40), "a".repeat(40)];
  const dataset = {
    id: "size",
    name: "Bundle size",
    columns: [
      {
        id: "gzip",
        name: "Gzipped size",
        unit: "B",
        direction: "DIRECTION_LOWER",
        percentageThreshold: "2",
      },
    ],
  };
  const executions = [0, 2].map((index) => ({
    context: {
      repositoryId: "42",
      executionId: `fixture-${index}`,
      commit: commits[index],
      branch: "main",
    },
    state: "EXECUTION_STATE_COMPLETED",
    representative: true,
    registeredAt: `2026-10-${20 - index}T00:00:00Z`,
    completedAt: `2026-10-${20 - index}T00:01:00Z`,
    registrationOrder: String(3 - index),
    measurements: [
      {
        dataset,
        rows: [
          {
            id: "jquery",
            name: "jQuery",
            values: { gzip: index === 0 ? "30321" : "32000" },
          },
          {
            id: "three",
            name: "Three.js",
            values: { gzip: index === 0 ? "170000" : "165000" },
          },
        ],
      },
    ],
  }));
  const comparisons = [
    {
      datasetId: "size",
      rowId: "three",
      columnId: "gzip",
      value: "170000",
      baselineValue: "165000",
      delta: "5000",
      percentage: "3.03030303",
      status: "COMPARISON_STATUS_REGRESSION",
      baselineCommit: commits[2],
      baselineExecutionId: "fixture-2",
      unit: "B",
    },
  ];
  window.fetch = async (input, init) => {
    const url = String(input);
    const request = JSON.parse(
      typeof init?.body === "string"
        ? init.body
        : init?.body
          ? new TextDecoder().decode(init.body)
          : "{}",
    );
    let body;
    if (url.endsWith("/ListRepositories"))
      body = {
        repositories: [
          {
            id: "42",
            spaceId: "fixture-space",
            fullName: "example/compiler",
            public: true,
            connected: true,
          },
        ],
      };
    else if (url.endsWith("/GetHistory")) body = { executions, commits };
    else if (url.endsWith("/GetExecution"))
      body = {
        execution: executions.find(
          (e) => e.context.executionId === request.executionId,
        ),
        comparisons,
      };
    else if (url.endsWith("/Compare")) body = { comparisons };
    else return original(input, init);
    return new Response(JSON.stringify(body), {
      headers: {
        "Content-Type": "application/json",
        "X-Correlation-Id": "browser-fixture",
      },
    });
  };
  return "Mock Connect API installed for browser verification";
}
