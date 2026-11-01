// SPDX-License-Identifier: FSL-1.1-ALv2

// Package migrations embeds the ordered SQL migrations of mcpa-api.
//
// Files are named NNNN_description.sql. Numbers are contiguous from 0001,
// except for the reserved numbers listed in store.reservedMigrations.
package migrations

import "embed"

// FS holds every migration file.
//
//go:embed *.sql
var FS embed.FS
