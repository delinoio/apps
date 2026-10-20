-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Keep accepted price evidence until the owning tenant passes accounting retention.
-- Retention deletes these rows before usage_events so the trigger can verify ownership.
CREATE FUNCTION immutable_ai_price() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' AND current_setting('delcore.retention',true)='on'
 AND EXISTS(SELECT 1 FROM usage_events u JOIN tenants t ON t.id=u.tenant_id
            WHERE u.id=OLD.usage_id AND t.deleted_at IS NOT NULL)
 THEN RETURN OLD; END IF;
 RAISE EXCEPTION 'AI price records are append-only';
END $$;
CREATE TRIGGER ai_prices_immutable BEFORE UPDATE OR DELETE ON ai_usage_prices
 FOR EACH ROW EXECUTE FUNCTION immutable_ai_price();
