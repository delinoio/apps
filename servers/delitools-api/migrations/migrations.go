// SPDX-License-Identifier: FSL-1.1-ALv2
package migrations

import "embed"

// Files holds forward-only migrations.
//
//go:embed *.sql
var Files embed.FS

const Version = 3
