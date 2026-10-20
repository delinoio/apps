-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Refuse ambiguous existing assignments; never silently discard billing evidence.
CREATE UNIQUE INDEX attempts_generation_unique ON attempts(generation_id) WHERE generation_id <> '';
