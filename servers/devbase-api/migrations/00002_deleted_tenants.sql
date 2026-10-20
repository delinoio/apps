-- deleted_tenants records tenants whose data cleanup finished. A request that was authorized
-- while the tenant was still active must not create projects after that cleanup. The row holds
-- only the tenant ID and is kept because Delcore never reuses tenant IDs.
CREATE TABLE deleted_tenants (
    tenant_id  uuid PRIMARY KEY,
    deleted_at timestamptz NOT NULL
);
