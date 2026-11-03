// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"net/http"
	"strings"

	"github.com/google/uuid"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
)

// Kimi device headers.
//
// Workaround: the pinned SDK Kimi executor sets X-Msh-Device-Name to the
// server host name, X-Msh-Device-Model to "<GOOS> <GOARCH>" and, unless the
// credential carries a device ID, X-Msh-Device-Id to an ID stored in a file
// under the server home (internal/runtime/executor/kimi_executor.go,
// applyKimiHeaders, SDK v6.10.9). Those values would disclose the host name
// and platform to the provider and link every credential and tenant on this
// server through one host-level device ID. mcpa identifies itself instead,
// with the same name and model as its Kimi device login
// (provideroauth/providers.go), and a device ID of the credential. The values
// are replaced in meteringTransport.RoundTrip, the one place every
// OpenAI-format Kimi request passes through; Claude-format Kimi requests go
// through the SDK Claude executor, which sends no X-Msh-Device headers.
//
// Scope: Kimi provider requests that already carry X-Msh-Device-Name, which
// is the SDK's OpenAI-format path. Remove this rewrite when the SDK stops
// sending host details or accepts the device name, model and ID through the
// executor configuration.
const (
	kimiHeaderDeviceName  = "X-Msh-Device-Name"
	kimiHeaderDeviceModel = "X-Msh-Device-Model"
	kimiHeaderDeviceID    = "X-Msh-Device-Id"

	kimiDeviceName  = "Managed CPA"
	kimiDeviceModel = "mcpa-api"
)

// kimiDeviceNamespace derives per-credential device IDs for credentials
// without a stored one. Its value is fixed so that IDs stay stable across
// restarts and deployments.
var kimiDeviceNamespace = uuid.MustParse("5b0a6f3e-8d1c-5c7e-9a43-2f6b1d0e7a11")

// kimiDeviceIDFor returns the device ID of a Kimi credential: the ID stored in
// its OAuth payload (created by the device login) or a fixed UUID derived
// from the credential ID.
func kimiDeviceIDFor(auth *coreauth.Auth) string {
	if auth == nil {
		return ""
	}
	if stored, ok := auth.Metadata["device_id"].(string); ok {
		if stored = strings.TrimSpace(stored); stored != "" {
			return stored
		}
	}
	credentialID := auth.Attributes[attrCredentialID]
	if credentialID == "" {
		return ""
	}
	return uuid.NewSHA1(kimiDeviceNamespace, []byte(credentialID)).String()
}

// kimiDeviceHeaderRewrite returns the function that replaces the device
// headers of req's clone, or a no-op for requests outside the workaround's
// scope.
func kimiDeviceHeaderRewrite(call *callState, req *http.Request) func(http.Header) {
	deviceID, ok := call.kimiDevice()
	if !ok || req.Header.Get(kimiHeaderDeviceName) == "" {
		return func(http.Header) {}
	}
	return func(header http.Header) {
		header.Set(kimiHeaderDeviceName, kimiDeviceName)
		header.Set(kimiHeaderDeviceModel, kimiDeviceModel)
		header.Set(kimiHeaderDeviceID, deviceID)
	}
}
