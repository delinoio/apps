-- SPDX-License-Identifier: FSL-1.1-ALv2
ALTER TABLE requests ADD COLUMN error_reason integer NOT NULL DEFAULT 0;
