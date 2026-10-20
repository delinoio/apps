-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Retain earlier prices for requests whose authorization overlaps synchronization.
ALTER TABLE catalog DROP CONSTRAINT catalog_pkey;
ALTER TABLE catalog ADD PRIMARY KEY (model, effective_at);
