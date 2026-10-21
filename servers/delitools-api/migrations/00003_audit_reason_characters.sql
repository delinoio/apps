-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Operator reasons are bounded to 1000 characters (not bytes), matching the API validation.
-- The foundation constraint counted bytes, so a valid multibyte reason failed as an internal
-- storage error. Forward-only fix: replace the constraint.
ALTER TABLE audit_records DROP CONSTRAINT audit_records_reason_check;
ALTER TABLE audit_records
    ADD CONSTRAINT audit_records_reason_check CHECK (char_length(reason) <= 1000);
