// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  destinations,
  DestinationKind,
  filterDestinations,
  nextDestination,
} from "../src/navigation.ts";

test("navigation searches display names and canonical repository URLs without losing stable identities", () => {
  const items = destinations([
    { id: "repo-one", name: "Frontend", url: "github.com/delinoio/apps" },
    { id: "repo-two", name: "한국어", url: "github.com/example/docs" },
  ]);
  assert.equal(items[0].label, "All tasks");
  assert.equal(items[5].kind, DestinationKind.App);
  assert.deepEqual(
    filterDestinations(items, "  DELINOIO front  ").map((item) => item.target),
    ["repo-one"],
  );
  assert.deepEqual(
    filterDestinations(items, "한국어").map((item) => item.target),
    ["repo-two"],
  );
  assert.equal(filterDestinations(items, "missing destination").length, 0);
  assert.deepEqual(filterDestinations(items, " \t "), items);
  assert.equal(destinations([]).length, 8);
});

test("keyboard selection wraps and handles an empty result set", () => {
  assert.equal(nextDestination(0, -1, 9), 8);
  assert.equal(nextDestination(8, 1, 9), 0);
  assert.equal(nextDestination(0, 1, 0), 0);
});
