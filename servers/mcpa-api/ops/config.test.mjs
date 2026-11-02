// SPDX-License-Identifier: FSL-1.1-ALv2
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import yaml from "js-yaml";

const text = (name) => readFileSync(new URL(name, import.meta.url), "utf8");
const read = (name) => yaml.load(text(name));
const rules = (name) => read(name).groups.flatMap((group) => group.rules);
// The server's exported metric families. Rules and dashboards must not invent others.
const metrics = new Set([
  "mcpa_calls_total",
  "mcpa_call_duration_seconds_bucket",
  "mcpa_provider_failures_total",
  "mcpa_delcore_failures_total",
  "mcpa_outbox_pending",
  "mcpa_outbox_poisoned",
  "mcpa_outbox_oldest_pending_age_seconds",
  "mcpa_outbox_delivery_failures_total",
  "mcpa_metering_checkpoint_failures_total",
  "mcpa_budget_rejections_total",
  "mcpa_billable_bytes_total",
  "mcpa_http_requests_total",
]);
const dashboard = JSON.parse(text("dashboard.json"));
const queries = [
  ...["recording.yml", "alerts.yml"].flatMap((name) =>
    rules(name).map((rule) => rule.expr),
  ),
  ...dashboard.panels.flatMap((panel) =>
    panel.targets.map((target) => target.expr),
  ),
];

test("every alert threshold comes from the operator threshold file", () => {
  const thresholds = rules("thresholds.yml");
  const names = new Set(thresholds.map((rule) => rule.record));
  for (const rule of thresholds) {
    assert.match(rule.record, /^mcpa:threshold:[a-z0-9_]+$/u);
    assert.match(rule.expr, /^vector\(\d+(?:\.\d+)?\)$/u, rule.record);
  }
  const used = new Set();
  for (const rule of rules("alerts.yml")) {
    const references = rule.expr.match(/mcpa:threshold:[a-z0-9_]+/gu) ?? [];
    if (rule.alert !== "McpaTargetDown")
      assert.ok(references.length > 0, `${rule.alert} has no threshold`);
    // Literal comparisons would bypass the operator configuration.
    assert.doesNotMatch(
      rule.expr.replace(/\{[^}]*\}/gu, ""),
      /[<>]=?\s*\d/u,
      rule.alert,
    );
    for (const reference of references) {
      assert.ok(names.has(reference), reference);
      used.add(reference);
    }
  }
  assert.deepEqual(
    [...names].filter((name) => !used.has(name)),
    [],
  );
});

test("alerts cover every required operational signal", () => {
  assert.deepEqual(
    rules("alerts.yml").map((rule) => rule.alert),
    [
      "McpaTargetDown",
      "McpaCallErrors",
      "McpaProviderFailures",
      "McpaDelcoreAuthorizationFailures",
      "McpaOutboxBacklog",
      "McpaOutboxAge",
      "McpaOutboxPoisoned",
      "McpaMeteringCheckpointFailures",
      "McpaBudgetRejections",
    ],
  );
  for (const rule of rules("alerts.yml"))
    assert.ok(
      ["critical", "warning", "info"].includes(rule.labels.severity),
      rule.alert,
    );
});

test("queries use exported metrics with bounded labels only", () => {
  for (const query of queries) {
    for (const name of query.match(/\bmcpa_[a-z0-9_]+/gu) ?? [])
      assert.ok(metrics.has(name), `${name} in ${query}`);
    // Tenant, resource, key and model identities never become labels.
    assert.doesNotMatch(
      query,
      /\b(?:tenant|tenant_id|resource|resource_id|key|key_id|call_key|model|user|user_id)\b/u,
      query,
    );
  }
});

test("Prometheus loads every rule file and authenticates metrics with a secret file", () => {
  const config = read("prometheus.yml");
  assert.deepEqual(config.rule_files, [
    "/etc/mcpa-monitoring/thresholds.yml",
    "/etc/mcpa-monitoring/recording.yml",
    "/etc/mcpa-monitoring/alerts.yml",
  ]);
  assert.equal(config.global.evaluation_interval, "30s");
  const [scrape] = config.scrape_configs;
  assert.equal(scrape.job_name, "mcpa-api");
  assert.equal(scrape.authorization.type, "Bearer");
  assert.ok(scrape.authorization.credentials_file);
  assert.equal(scrape.authorization.credentials, undefined);
  for (const name of ["thresholds.yml", "recording.yml", "alerts.yml"])
    for (const group of read(name).groups)
      assert.equal(group.interval, config.global.evaluation_interval, name);
});

test("the dashboard uses the selected data source and unique panels", () => {
  assert.equal(dashboard.uid, "mcpa-operations");
  const ids = dashboard.panels.map((panel) => panel.id);
  assert.equal(new Set(ids).size, ids.length);
  for (const panel of dashboard.panels)
    assert.equal(panel.datasource.uid, "${DS_PROMETHEUS}", panel.title);
});
