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
  assert.equal(config.route.group_interval, "30s");
  assert.equal(config.route.repeat_interval, "1h");
  assert.equal(config.receivers[0].email_configs[0].send_resolved, true);
  assert.equal(config.global.smtp_require_tls, true);
  assert.ok(config.global.smtp_auth_password_file);
  assert.equal(config.global.smtp_auth_password, undefined);
});
test("probes use 30s cadence and 5s timeout; metrics require a separate bearer secret", () => {
  const config = read("prometheus.yml");
  assert.equal(config.global.scrape_interval, "30s");
  assert.equal(config.global.scrape_timeout, "5s");
  assert.equal(read("blackbox.yml").modules.lane_http.timeout, "5s");
  assert.equal(config.scrape_configs[0].authorization.type, "Bearer");
  assert.ok(config.scrape_configs[0].authorization.credentials_file);
  assert.equal(config.scrape_configs[1].scrape_timeout, "6s");
  assert.equal(config.scrape_configs[1].static_configs.length, 2);
});
