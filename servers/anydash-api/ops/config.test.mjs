// SPDX-License-Identifier: FSL-1.1-ALv2
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import yaml from "js-yaml";
const read = (name) =>
  yaml.load(readFileSync(new URL(name, import.meta.url), "utf8"));
test("email routing sends each alert immediately, hourly and on recovery", () => {
  const config = read("alertmanager.example.yml");
  assert.deepEqual(config.route.group_by, ["..."]);
  assert.equal(config.route.group_wait, "0s");
  assert.equal(config.route.repeat_interval, "1h");
  assert.equal(config.receivers[0].email_configs[0].send_resolved, true);
  assert.equal(config.global.smtp_require_tls, true);
  assert.ok(config.global.smtp_auth_password_file);
  assert.equal(config.global.smtp_auth_password, undefined);
});
test("metrics scraping uses a bearer secret file and no colliding target labels", () => {
  const config = read("prometheus.yml");
  const scrape = config.scrape_configs[0];
  assert.equal(scrape.authorization.type, "Bearer");
  assert.ok(scrape.authorization.credentials_file);
  assert.equal(scrape.authorization.credentials, undefined);
  for (const target of scrape.static_configs) {
    assert.equal(target.labels?.service, undefined);
  }
});
test("every alert required by the server contract exists", () => {
  const rules = read("alerts.yml").groups.flatMap((g) => g.rules);
  const names = rules.map((r) => r.alert);
  assert.deepEqual(names.sort(), [
    "AnyDashApiServerErrors",
    "AnyDashDeletionDeadlineAtRisk",
    "AnyDashPartialAggregation",
    "AnyDashPriceSyncStale",
    "AnyDashPriceValidationFailed",
    "AnyDashProviderCollectionFailures",
    "AnyDashWorkerJobFailed",
  ]);
  for (const rule of rules) {
    assert.ok(!/user_id|device_id|connection_id|tenant/u.test(rule.expr));
  }
});
