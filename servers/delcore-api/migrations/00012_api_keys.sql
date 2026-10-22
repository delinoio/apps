-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Reusable API keys. The secret is never stored: only the SHA-256 of its random
-- part is persisted. Owner kind 1 is a personal key (user_id owns it); owner
-- kind 2 is a tenant service key (user_id is the issuer, kept for audit only).
CREATE TABLE api_keys (
 id uuid PRIMARY KEY,
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 owner_kind smallint NOT NULL CHECK(owner_kind IN (1,2)),
 user_id uuid NOT NULL REFERENCES users(id),
 name text NOT NULL CHECK(char_length(name) BETWEEN 1 AND 100),
 secret_hash bytea NOT NULL CHECK(octet_length(secret_hash)=32),
 created_at timestamptz NOT NULL,
 expires_at timestamptz,
 revoked_at timestamptz,
 CHECK(expires_at IS NULL OR expires_at>created_at)
);
CREATE INDEX api_keys_owner ON api_keys(tenant_id,owner_kind,user_id,id);
CREATE INDEX api_keys_user ON api_keys(user_id);

-- Grants are fixed at creation: one row per registered service.
CREATE TABLE api_key_grants (
 key_id uuid NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
 service_id uuid NOT NULL REFERENCES services(id),
 capabilities text[] NOT NULL CHECK(cardinality(capabilities) BETWEEN 1 AND 32),
 PRIMARY KEY(key_id,service_id)
);

-- Only revocation may change a key. Grants, expiry and the secret hash stay
-- fixed; a replacement key carries any change.
CREATE FUNCTION api_key_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='api_key_grants' THEN
  RAISE EXCEPTION 'API key grants are immutable';
 END IF;
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
 OR NEW.owner_kind IS DISTINCT FROM OLD.owner_kind OR NEW.user_id IS DISTINCT FROM OLD.user_id
 OR NEW.name IS DISTINCT FROM OLD.name OR NEW.secret_hash IS DISTINCT FROM OLD.secret_hash
 OR NEW.created_at IS DISTINCT FROM OLD.created_at OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
 OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at) THEN
  RAISE EXCEPTION 'API key fields are immutable';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER api_keys_immutable BEFORE UPDATE ON api_keys
 FOR EACH ROW EXECUTE FUNCTION api_key_immutable();
CREATE TRIGGER api_key_grants_immutable BEFORE UPDATE ON api_key_grants
 FOR EACH ROW EXECUTE FUNCTION api_key_immutable();
