-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Semantic versions can be reactivated (A -> B -> A). Their creation times
-- cannot identify the price effective at an execution's start.
CREATE TABLE catalog_price_activations (
 price_id uuid NOT NULL REFERENCES catalog_entities(id),
 version_id uuid NOT NULL,
 effective_at timestamptz NOT NULL,
 PRIMARY KEY(price_id,effective_at),
 FOREIGN KEY(version_id,price_id) REFERENCES catalog_versions(id,entity_id)
);
-- Start the new policy at migration time; do not invent earlier activations.
INSERT INTO catalog_price_activations(price_id,version_id,effective_at)
 SELECT id,active_version_id,clock_timestamp() FROM catalog_entities
 WHERE kind='price' AND active_version_id IS NOT NULL;

CREATE FUNCTION record_price_activation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.kind='price' AND NEW.active_version_id IS NOT NULL
 AND NEW.active_version_id IS DISTINCT FROM OLD.active_version_id THEN
  INSERT INTO catalog_price_activations(price_id,version_id,effective_at)
   VALUES(NEW.id,NEW.active_version_id,clock_timestamp());
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER catalog_price_activation AFTER UPDATE OF active_version_id ON catalog_entities
 FOR EACH ROW EXECUTE FUNCTION record_price_activation();
CREATE TRIGGER catalog_price_activations_immutable BEFORE UPDATE OR DELETE ON catalog_price_activations
 FOR EACH ROW EXECUTE FUNCTION immutable_catalog_version();

CREATE INDEX requests_created_at ON requests(created_at);
CREATE INDEX audit_created_at ON audit(created_at);
