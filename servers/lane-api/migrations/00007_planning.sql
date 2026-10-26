-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Repository planning items: labels (4), milestones (5) and cycles (6) are account-scoped
-- entities with the same immutable-version, head and tombstone machinery as tasks. No
-- immutable envelope, receipt fingerprint or version identity is rewritten.
ALTER TABLE lane_entities DROP CONSTRAINT lane_entities_kind_check;
ALTER TABLE lane_entities ADD CONSTRAINT lane_entities_kind_check CHECK (kind IN (1,2,3,4,5,6));
-- Planning items by account, repository and kind (name checks, listing, cleanup on deletion).
CREATE INDEX lane_planning_items ON lane_entities(account_id,repository_id,kind) WHERE kind IN (4,5,6) AND NOT deleted;
-- Planning listing by kind in stable entity-id order.
CREATE INDEX lane_planning_by_kind ON lane_entities(account_id,kind,id) WHERE kind IN (4,5,6) AND NOT deleted;
-- Live tasks in stable id order for filtered, sorted and keyset-paged task lists.
CREATE INDEX lane_live_tasks ON lane_entities(account_id,id) WHERE kind=2 AND NOT deleted;
