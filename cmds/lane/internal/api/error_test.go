// SPDX-License-Identifier: Apache-2.0
package api

import (
	"connectrpc.com/connect"
	"errors"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"strings"
	"testing"
)

func TestConflictFailurePreservesTypedAlternatives(t *testing.T) {
	err := connect.NewError(connect.CodeAborted, errors.New("unsafe diagnostic"))
	detail, _ := connect.NewErrorDetail(&v1.ErrorDetail{Reason: v1.ErrorReason_ERROR_REASON_STALE_VERSION, EntityId: "entity", CurrentHeadIds: []string{"a", "b"}})
	err.AddDetail(detail)
	result, _ := connect.NewErrorDetail(&v1.EntityResult{Result: v1.ResultKind_RESULT_KIND_CONFLICT, Alternatives: []*v1.EntityVersion{{VersionId: "a"}, {VersionId: "b"}}})
	err.AddDetail(result)
	failure := Error(err)
	if failure.EntityID != "entity" || len(failure.CurrentHeadIDs) != 2 || !strings.Contains(string(failure.Conflict), `"versionId":"b"`) || strings.Contains(failure.Message, "unsafe") {
		t.Fatalf("lost or unsafe failure: %+v", failure)
	}
}
