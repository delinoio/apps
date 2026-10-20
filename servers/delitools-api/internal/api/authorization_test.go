// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	"fmt"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"testing"
)

func TestAuthorizationCancellation(t *testing.T) {
	for _, tc := range []struct {
		input  error
		code   connect.Code
		reason pb.ErrorReason
	}{
		{connect.NewError(connect.CodeCanceled, errors.New("canceled")), connect.CodeCanceled, pb.ErrorReason_ERROR_REASON_CANCELED},
		{connect.NewError(connect.CodeDeadlineExceeded, errors.New("deadline")), connect.CodeDeadlineExceeded, pb.ErrorReason_ERROR_REASON_TIMEOUT},
		{fmt.Errorf("wrapped: %w", context.Canceled), connect.CodeCanceled, pb.ErrorReason_ERROR_REASON_CANCELED},
		{fmt.Errorf("wrapped: %w", context.DeadlineExceeded), connect.CodeDeadlineExceeded, pb.ErrorReason_ERROR_REASON_TIMEOUT},
	} {
		err := authFault(tc.input, "request")
		if connect.CodeOf(err) != tc.code {
			t.Fatal(err)
		}
		var ce *connect.Error
		if !errors.As(err, &ce) || len(ce.Details()) != 1 {
			t.Fatal(err)
		}
		value, e := ce.Details()[0].Value()
		if e != nil || value.(*pb.ErrorDetail).Reason != tc.reason {
			t.Fatal(value, e)
		}
	}
}
