// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"errors"
	"strings"
	"testing"

	"connectrpc.com/connect"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
)

func TestValidateFilterRejects(t *testing.T) {
	many := make([]string, 101)
	for i := range many {
		many[i] = "m"
	}
	cases := map[string]struct {
		f     *mcpav1.CallFilter
		field string
	}{
		"nul":         {&mcpav1.CallFilter{Models: []string{"a\x00b"}}, "filter.models"},
		"control":     {&mcpav1.CallFilter{Models: []string{"a\nb"}}, "filter.models"},
		"empty":       {&mcpav1.CallFilter{Models: []string{""}}, "filter.models"},
		"long":        {&mcpav1.CallFilter{Models: []string{strings.Repeat("a", 201)}}, "filter.models"},
		"invalidutf8": {&mcpav1.CallFilter{Models: []string{"\xff"}}, "filter.models"},
		"toomany":     {&mcpav1.CallFilter{Models: many}, "filter.models"},
		"prov65537":   {&mcpav1.CallFilter{Providers: []mcpav1.Provider{65537}}, "filter.providers"},
		"provneg":     {&mcpav1.CallFilter{Providers: []mcpav1.Provider{-65535}}, "filter.providers"},
		"prov99":      {&mcpav1.CallFilter{Providers: []mcpav1.Provider{99}}, "filter.providers"},
		"status65537": {&mcpav1.CallFilter{Statuses: []mcpav1.CallStatus{65537}}, "filter.statuses"},
	}
	for name, c := range cases {
		err := validateFilter(c.f)
		if err == nil {
			t.Fatalf("%s: expected error", name)
		}
		var ae *apperr.Error
		if !errors.As(err, &ae) || ae.Code != connect.CodeInvalidArgument || ae.Reason != mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT {
			t.Fatalf("%s: got %v, want INVALID_INPUT", name, err)
		}
		if !strings.Contains(err.Error(), c.field) {
			t.Fatalf("%s: error %q does not name %s", name, err, c.field)
		}
	}
}

func TestValidateFilterAccepts(t *testing.T) {
	f := &mcpav1.CallFilter{
		Models:    []string{strings.Repeat("a", 200), "gpt", "gpt"},
		Providers: []mcpav1.Provider{mcpav1.Provider_PROVIDER_UNSPECIFIED, mcpav1.Provider_PROVIDER_GEMINI, mcpav1.Provider_PROVIDER_GEMINI},
		Statuses:  []mcpav1.CallStatus{mcpav1.CallStatus_CALL_STATUS_SUCCEEDED, mcpav1.CallStatus_CALL_STATUS_UNSPECIFIED},
	}
	if err := validateFilter(f); err != nil {
		t.Fatal(err)
	}
}
