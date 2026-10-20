// SPDX-License-Identifier: FSL-1.1-ALv2

package postgres

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

var ErrCatalogIdentity = errors.New("catalog identity conflicts with its stable key")

func catalogID() string { return uuid.Must(uuid.NewV7()).String() }

// EnsureCatalogEntity keeps a stable UUID for a kind, scope, and key. Callers
// must use the same transaction for entity data and version selection.
func EnsureCatalogEntity(ctx context.Context, tx pgx.Tx, kind, scope, key, requestedID string) (string, error) {
	var id string
	err := tx.QueryRow(ctx, "SELECT id::text FROM catalog_entities WHERE kind=$1 AND scope=$2 AND key=$3 FOR UPDATE", kind, scope, key).Scan(&id)
	if err == nil {
		if requestedID != "" && requestedID != id {
			return "", ErrCatalogIdentity
		}
		return id, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return "", err
	}
	if requestedID == "" {
		requestedID = catalogID()
	}
	_, err = tx.Exec(ctx, "INSERT INTO catalog_entities(id,kind,scope,key) VALUES($1,$2,$3,$4)", requestedID, kind, scope, key)
	return requestedID, err
}

func CatalogKey(ctx context.Context, tx pgx.Tx, id string) (string, error) {
	var key string
	err := tx.QueryRow(ctx, "SELECT key FROM catalog_entities WHERE id=$1", id).Scan(&key)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", nil
	}
	return key, err
}

// SelectCatalogVersion hashes canonical JSON content and reselects a prior
// immutable version when a definition returns from B to A.
func SelectCatalogVersion(ctx context.Context, tx pgx.Tx, entityID string, definition any) (string, bool, error) {
	data, err := json.Marshal(definition)
	if err != nil {
		return "", false, err
	}
	sum := sha256.Sum256(data)
	hash := hex.EncodeToString(sum[:])
	var id string
	err = tx.QueryRow(ctx, "SELECT id::text FROM catalog_versions WHERE entity_id=$1 AND semantic_hash=$2", entityID, hash).Scan(&id)
	created := false
	if errors.Is(err, pgx.ErrNoRows) {
		id = catalogID()
		_, err = tx.Exec(ctx, "INSERT INTO catalog_versions(id,entity_id,semantic_hash,definition) VALUES($1,$2,$3,$4)", id, entityID, hash, data)
		created = err == nil
	}
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		return "", false, err
	}
	_, err = tx.Exec(ctx, "UPDATE catalog_entities SET active_version_id=$2 WHERE id=$1", entityID, id)
	return id, created, err
}

func ActiveCatalogVersion(ctx context.Context, tx pgx.Tx, kind, id string) (string, error) {
	var version *string
	err := tx.QueryRow(ctx, "SELECT active_version_id::text FROM catalog_entities WHERE id=$1 AND kind=$2", id, kind).Scan(&version)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", nil
	}
	if err != nil || version == nil {
		return "", err
	}
	return *version, nil
}

func ActiveProductVersions(ctx context.Context, tx pgx.Tx, plan string, addons []string) ([]string, error) {
	versions := []string{}
	base, err := ActiveCatalogVersion(ctx, tx, "plan", plan)
	if err != nil {
		return nil, err
	}
	if base != "" {
		versions = append(versions, base)
	}
	sort.Strings(addons)
	for _, addon := range addons {
		version, err := ActiveCatalogVersion(ctx, tx, "addon", addon)
		if err != nil {
			return nil, err
		}
		if version != "" {
			versions = append(versions, version)
		}
	}
	return versions, nil
}

func ConflictingFeature(ctx context.Context, tx pgx.Tx, versions []string) (bool, error) {
	if len(versions) < 2 {
		return false, nil
	}
	var conflict bool
	err := tx.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM catalog_version_features
 WHERE version_id=ANY($1::uuid[]) GROUP BY feature_id HAVING count(*)>1)`, versions).Scan(&conflict)
	return conflict, err
}

// ActiveProductPrices follows the selected price version for each stable price
// key. Legacy rows without catalog identities retain their prior newest-row
// behavior until the operator declares a stable key.
func ActiveProductPrices(ctx context.Context, tx pgx.Tx, ownerKind, ownerID string) ([]string, error) {
	column := "plan_id"
	if ownerKind == "addon" {
		column = "addon_id"
	}
	rows, err := tx.Query(ctx, `SELECT active_version_id::text FROM catalog_entities WHERE kind='price' AND scope=$2 AND active_version_id IS NOT NULL
 UNION ALL
 SELECT id::text FROM (SELECT DISTINCT ON (p.service_id,p.meter,p.kind) p.id
 FROM prices p LEFT JOIN catalog_versions cv ON cv.id=p.id
 WHERE p.`+column+`=$1 AND cv.id IS NULL
 ORDER BY p.service_id,p.meter,p.kind,p.created_at DESC,p.id DESC) legacy`, ownerID, ownerKind+":"+ownerID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var prices []string
	for rows.Next() {
		var price string
		if err = rows.Scan(&price); err != nil {
			return nil, err
		}
		prices = append(prices, price)
	}
	return prices, rows.Err()
}

type versionFeature struct {
	ID    string          `json:"id"`
	Value json.RawMessage `json:"value"`
}
type productDefinition struct {
	Rank     int              `json:"rank"`
	Free     bool             `json:"free"`
	Postpaid bool             `json:"postpaid"`
	Prices   []string         `json:"prices"`
	Features []versionFeature `json:"features"`
}

// RebuildProductVersion records the current price and feature definitions as
// one selectable product version. Display names and source paths are excluded.
func RebuildProductVersion(ctx context.Context, tx pgx.Tx, kind, productID string) (string, error) {
	definition := productDefinition{Prices: []string{}, Features: []versionFeature{}}
	var ownerColumn string
	switch kind {
	case "plan":
		ownerColumn = "plan_id"
		if err := tx.QueryRow(ctx, "SELECT rank,free,postpaid FROM plans WHERE id=$1", productID).Scan(&definition.Rank, &definition.Free, &definition.Postpaid); err != nil {
			return "", err
		}
	case "addon":
		ownerColumn = "addon_id"
	default:
		return "", fmt.Errorf("unknown product kind %q", kind)
	}
	rows, err := tx.Query(ctx, `SELECT active_version_id::text FROM catalog_entities WHERE kind='price' AND scope=$2 AND active_version_id IS NOT NULL
 UNION ALL
 SELECT id::text FROM (SELECT DISTINCT ON (p.service_id,p.meter,p.kind) p.id
 FROM prices p LEFT JOIN catalog_versions cv ON cv.id=p.id
 WHERE p.`+ownerColumn+`=$1 AND cv.id IS NULL
 ORDER BY p.service_id,p.meter,p.kind,p.created_at DESC,p.id DESC) legacy`, productID, kind+":"+productID)
	if err != nil {
		return "", err
	}
	for rows.Next() {
		var id string
		if err = rows.Scan(&id); err != nil {
			rows.Close()
			return "", err
		}
		definition.Prices = append(definition.Prices, id)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return "", err
	}
	sort.Strings(definition.Prices)
	rows, err = tx.Query(ctx, "SELECT feature_id::text,value FROM catalog_product_features WHERE product_id=$1 ORDER BY feature_id", productID)
	if err != nil {
		return "", err
	}
	for rows.Next() {
		var feature versionFeature
		if err = rows.Scan(&feature.ID, &feature.Value); err != nil {
			rows.Close()
			return "", err
		}
		definition.Features = append(definition.Features, feature)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return "", err
	}
	version, created, err := SelectCatalogVersion(ctx, tx, productID, definition)
	if err != nil {
		return "", err
	}
	if created {
		for _, feature := range definition.Features {
			if _, err = tx.Exec(ctx, "INSERT INTO catalog_version_features(version_id,feature_id,value) VALUES($1,$2,$3)", version, feature.ID, feature.Value); err != nil {
				return "", err
			}
		}
	}
	return version, nil
}

// ReplaceTenantSelections records the versions chosen by an explicit change.
// Empty versions are permitted only for legacy rows created outside catalog APIs.
func ReplaceTenantSelections(ctx context.Context, tx pgx.Tx, tenant string, versions []string, at time.Time) error {
	if _, err := tx.Exec(ctx, "DELETE FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL AND starts_at>=$2", tenant, at); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, "UPDATE tenant_catalog_selections SET ends_at=$2 WHERE tenant_id=$1 AND ends_at IS NULL AND starts_at<$2", tenant, at); err != nil {
		return err
	}
	for _, version := range versions {
		if version == "" {
			continue
		}
		var product string
		if err := tx.QueryRow(ctx, "SELECT entity_id::text FROM catalog_versions WHERE id=$1", version).Scan(&product); err != nil {
			return err
		}
		if _, err := tx.Exec(ctx, "INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at) VALUES($1,$2,$3,$4,$5)", catalogID(), tenant, product, version, at); err != nil {
			return err
		}
	}
	return nil
}
