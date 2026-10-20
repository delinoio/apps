-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE INDEX attempts_request_order ON attempts(request_id, created_at, id);
