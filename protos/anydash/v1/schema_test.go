// SPDX-License-Identifier: Apache-2.0
package anydashv1_test

import (
	"strings"
	"testing"

	"google.golang.org/protobuf/reflect/protoreflect"

	anydashv1 "github.com/delinoio/apps/protos/anydash/v1"
)

// TestEnumZeroValues checks the protocol contract rule that every enum zero
// value is <ENUM>_UNSPECIFIED and that values are consecutive from zero.
func TestEnumZeroValues(t *testing.T) {
	file := anydashv1.File_anydash_v1_anydash_proto
	enums := file.Enums()
	if enums.Len() == 0 {
		t.Fatal("schema declares no enums")
	}
	for i := 0; i < enums.Len(); i++ {
		enum := enums.Get(i)
		values := enum.Values()
		zero := values.ByNumber(0)
		if zero == nil || !strings.HasSuffix(string(zero.Name()), "_UNSPECIFIED") {
			t.Errorf("%s: zero value must be *_UNSPECIFIED", enum.FullName())
		}
		for j := 0; j < values.Len(); j++ {
			if got := values.Get(j).Number(); got != protoreflect.EnumNumber(j) {
				t.Errorf("%s: value %d has number %d, want consecutive numbering", enum.FullName(), j, got)
			}
		}
	}
}

// TestFieldNumbersAreConsecutive checks the contract rule that message fields,
// including oneof members, are numbered consecutively from 1 in declaration order.
func TestFieldNumbersAreConsecutive(t *testing.T) {
	messages := anydashv1.File_anydash_v1_anydash_proto.Messages()
	for i := 0; i < messages.Len(); i++ {
		message := messages.Get(i)
		fields := message.Fields()
		for j := 0; j < fields.Len(); j++ {
			if got := fields.Get(j).Number(); got != protoreflect.FieldNumber(j+1) {
				t.Errorf("%s: field %s has number %d, want %d", message.FullName(), fields.Get(j).Name(), got, j+1)
			}
		}
	}
}

// TestServices checks the service and method names that the contract fixes.
func TestServices(t *testing.T) {
	want := map[string][]string{
		"DeviceService": {
			"StartEnrollment", "GetEnrollment", "ApproveEnrollment", "RejectEnrollment",
			"PollEnrollment", "RefreshDeviceToken", "RevokeDeviceCredentials", "ListDevices",
			"DeleteDevice", "SetPreferredDevice", "RequestDeviceSync",
		},
		"CollectorService": {
			"Heartbeat", "UploadUsage", "ReportParserStatus", "AcquireLease", "RenewLease",
			"ReleaseLease", "ReportSnapshot",
		},
		"ConnectionService": {
			"CreateConnection", "ListConnections", "GetConnection", "DeleteConnection",
			"RegisterConnectionDevice", "UnregisterConnectionDevice", "RequestConnectionRefresh",
		},
		"DashboardService": {
			"ListSpaces", "ListDashboards", "GetDashboard", "CreateDashboard", "RenameDashboard",
			"DeleteDashboard", "AddWidget", "RemoveWidget", "UpdateWidgetLayout", "UpdateWidgetQuery",
			"SetWidgetSharingScope", "RevokeWidgetSharing",
		},
		"WidgetDataService": {"QueryWidgetData", "QueryDashboardData"},
		"AccountService": {
			"ConnectAccount", "GetAccount", "UpdateAccountSettings", "ListActivity", "DeleteAccountData",
		},
		"LocalCollectorService": {
			"GetCollectorStatus", "StartLogin", "Logout", "AddConnectionCredential",
			"RemoveConnectionCredential", "ListSourcePaths", "AddSourcePath", "DisableSourcePath",
			"EnableSourcePath", "RotateKey", "RequestLocalSync", "RequestLocalConnectionRefresh",
			"PutNativeWidgetSnapshots", "ListNativeWidgetSnapshots", "Quit",
		},
	}
	services := anydashv1.File_anydash_v1_anydash_proto.Services()
	if services.Len() != len(want) {
		t.Fatalf("got %d services, want %d", services.Len(), len(want))
	}
	for name, methods := range want {
		service := services.ByName(protoreflect.Name(name))
		if service == nil {
			t.Errorf("missing service %s", name)
			continue
		}
		if service.Methods().Len() != len(methods) {
			t.Errorf("%s: got %d methods, want %d", name, service.Methods().Len(), len(methods))
			continue
		}
		for i, method := range methods {
			got := service.Methods().Get(i)
			if string(got.Name()) != method {
				t.Errorf("%s: method %d is %s, want %s", name, i, got.Name(), method)
			}
			if string(got.Input().Name()) != method+"Request" || string(got.Output().Name()) != method+"Response" {
				t.Errorf("%s.%s: request and response must be named %sRequest and %sResponse", name, method, method, method)
			}
		}
	}
}
