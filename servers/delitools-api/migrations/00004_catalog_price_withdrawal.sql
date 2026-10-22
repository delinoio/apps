-- SPDX-License-Identifier: FSL-1.1-ALv2
-- A catalog price version can record that a model is currently unpriced. When the public catalog
-- is read successfully but no active provider endpoint charges the published flat price, the model
-- has no applicable route or price, so the synchronization appends a withdrawal version instead of
-- leaving the previous price in force. A failed catalog fetch never withdraws a price. A withdrawal
-- row carries zero prices that no request ever uses; the lookup treats a newest version with
-- priced = false as "no price". Forward-only; recovery is fix-forward.
ALTER TABLE catalog_prices ADD COLUMN priced boolean NOT NULL DEFAULT true;
