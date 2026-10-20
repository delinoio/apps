-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE INDEX requests_retention ON requests(created_at);
CREATE INDEX attempts_retention ON attempts(created_at);
CREATE INDEX audit_retention ON audit(created_at);
CREATE INDEX operational_logs_retention ON operational_logs(created_at);
