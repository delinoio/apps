-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Existing rows have no reliable entity identity and remain uncorrelated.
ALTER TABLE lane_sync_rejections
    ADD COLUMN epoch text,
    ADD COLUMN device_id text,
    ADD COLUMN entity_id text,
    ADD COLUMN version_id text,
    ADD COLUMN base_heads text[];
ALTER TABLE lane_sync_rejections
    ADD CONSTRAINT lane_sync_rejections_correlation_complete CHECK (
        (epoch IS NULL AND device_id IS NULL AND entity_id IS NULL AND version_id IS NULL AND base_heads IS NULL)
        OR (epoch IS NOT NULL AND device_id IS NOT NULL AND entity_id IS NOT NULL AND version_id IS NOT NULL AND base_heads IS NOT NULL)
    );
CREATE INDEX lane_sync_rejections_correction_idx
    ON lane_sync_rejections(account_id, epoch, device_id, entity_id, base_heads)
    WHERE entity_id IS NOT NULL;
