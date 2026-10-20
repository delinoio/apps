-- SPDX-License-Identifier: FSL-1.1-ALv2
ALTER TABLE attempts ADD COLUMN association_eligible boolean NOT NULL DEFAULT false;
-- Unknown attempts outlive their request row. Preserve existing recovery work,
-- including orphaned attempts whose shorter-lived request has already expired.
UPDATE attempts a SET association_eligible=true
WHERE a.generation_id='' AND a.usage IS NULL AND NOT a.delivered
  AND (EXISTS(SELECT 1 FROM requests r WHERE r.id=a.request_id AND r.state=3)
       OR NOT EXISTS(SELECT 1 FROM requests r WHERE r.id=a.request_id));
