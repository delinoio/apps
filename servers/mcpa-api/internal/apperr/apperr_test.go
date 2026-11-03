// SPDX-License-Identifier: FSL-1.1-ALv2

package apperr

import (
	"fmt"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

func TestAsMapsDatabaseTimeoutsToDependencyUnavailable(t *testing.T) {
	for _, code := range []string{"57014", "55P03"} {
		err := fmt.Errorf("query: %w", &pgconn.PgError{Code: code})
		if got := As(err); got.Reason != mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
			t.Errorf("SQLSTATE %s mapped to %s, want dependency unavailable", code, got.Reason)
		}
	}
	// Other database failures stay internal.
	if got := As(&pgconn.PgError{Code: "23505"}); got.Reason == mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
		t.Error("a unique violation must not map to dependency unavailable")
	}
}
