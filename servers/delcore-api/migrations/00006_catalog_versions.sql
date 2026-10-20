-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Identity is independent of both the source file and the currently selected
-- definition. A catalog version is immutable; moving the active pointer does
-- not change subscriptions that already selected another version.
CREATE TABLE catalog_entities (
 id uuid PRIMARY KEY,
 kind text NOT NULL CHECK (kind IN ('service','meter','feature','plan','addon','price')),
 scope text NOT NULL,
 key text NOT NULL,
 active_version_id uuid,
 UNIQUE(kind,scope,key),
 UNIQUE(id,kind)
);
CREATE TABLE catalog_versions (
 id uuid PRIMARY KEY,
 entity_id uuid NOT NULL REFERENCES catalog_entities(id),
 semantic_hash text NOT NULL,
 definition jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(entity_id,semantic_hash),
 UNIQUE(id,entity_id)
);
ALTER TABLE catalog_entities ADD CONSTRAINT catalog_active_version
 FOREIGN KEY(active_version_id,id) REFERENCES catalog_versions(id,entity_id);

CREATE TABLE features (
 id uuid PRIMARY KEY REFERENCES catalog_entities(id),
 service_id uuid NOT NULL REFERENCES services(id),
 key text NOT NULL,
 name text NOT NULL,
 kind integer NOT NULL CHECK(kind IN (1,2,3)),
 value_type integer NOT NULL CHECK(value_type BETWEEN 0 AND 5),
 meter_key text NOT NULL DEFAULT '',
 UNIQUE(service_id,key),
 CHECK ((kind=3 AND value_type<>0 AND meter_key='') OR
        (kind=2 AND value_type=0 AND meter_key<>'') OR
        (kind=1 AND value_type=0 AND meter_key=''))
);
CREATE TABLE catalog_product_features (
 product_id uuid NOT NULL REFERENCES catalog_entities(id),
 feature_id uuid NOT NULL REFERENCES features(id),
 value jsonb NOT NULL,
 PRIMARY KEY(product_id,feature_id)
);
CREATE TABLE catalog_version_features (
 version_id uuid NOT NULL REFERENCES catalog_versions(id),
 feature_id uuid NOT NULL REFERENCES features(id),
 value jsonb NOT NULL,
 PRIMARY KEY(version_id,feature_id)
);
CREATE FUNCTION immutable_catalog_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'catalog versions are immutable'; END $$;
CREATE TRIGGER catalog_versions_immutable BEFORE UPDATE OR DELETE ON catalog_versions
 FOR EACH ROW EXECUTE FUNCTION immutable_catalog_version();
CREATE TRIGGER catalog_version_features_immutable BEFORE UPDATE OR DELETE ON catalog_version_features
 FOR EACH ROW EXECUTE FUNCTION immutable_catalog_version();

CREATE TABLE tenant_catalog_selections (
 id uuid PRIMARY KEY,
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 product_id uuid NOT NULL REFERENCES catalog_entities(id),
 version_id uuid NOT NULL REFERENCES catalog_versions(id),
 starts_at timestamptz NOT NULL,
 ends_at timestamptz,
 CHECK (ends_at IS NULL OR ends_at>starts_at)
);
CREATE UNIQUE INDEX tenant_catalog_active_product ON tenant_catalog_selections(tenant_id,product_id)
 WHERE ends_at IS NULL;
CREATE INDEX tenant_catalog_history ON tenant_catalog_selections(tenant_id,starts_at,ends_at);
