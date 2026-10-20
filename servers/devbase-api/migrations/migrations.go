// SPDX-License-Identifier: FSL-1.1-ALv2

package migrations

import "embed"

// Files contains ordered, immutable schema migrations. Add a new file and raise
// Version instead of editing an applied migration; the checksum guard rejects edits.
//
//go:embed *.sql
var Files embed.FS

const Version = 2
