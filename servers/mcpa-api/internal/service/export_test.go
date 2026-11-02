// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import "time"

// SetClock replaces the service clock in tests.
func (s *Service) SetClock(now func() time.Time) { s.now = now }
