-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Raw event attribute maps follow raw-usage retention after a tenant's
-- deletion is final, while revision identities, fingerprints, contributions,
-- and price attributions remain accounting evidence until the later
-- accounting cutoff. Controlled retention may therefore clear only the
-- attribute map of a revision of a deleted tenant; every other update stays
-- rejected.
CREATE OR REPLACE FUNCTION metering_history_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF current_setting('delcore.retention',true)='on'
 AND EXISTS(SELECT 1 FROM tenants WHERE id=OLD.tenant_id AND deleted_at IS NOT NULL) THEN
  IF TG_OP='DELETE' THEN RETURN OLD; END IF;
  -- Nested conditions keep NEW.attributes away from the contributions table.
  IF TG_TABLE_NAME='metering_event_revisions' THEN
   IF NEW.attributes='{}'::jsonb
   AND ROW(NEW.event_id,NEW.tenant_id,NEW.revision,NEW.kind,NEW.event_type,NEW.fingerprint,NEW.created_at)
    IS NOT DISTINCT FROM ROW(OLD.event_id,OLD.tenant_id,OLD.revision,OLD.kind,OLD.event_type,OLD.fingerprint,OLD.created_at)
   THEN RETURN NEW; END IF;
  END IF;
 END IF;
 RAISE EXCEPTION 'metering event history is append-only';
END $$;
