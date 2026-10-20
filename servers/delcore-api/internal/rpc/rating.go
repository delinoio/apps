// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"time"
)

func (c *call) ratePeriod(tenant, service, meter string, start time.Time) (accounting.Decimal, accounting.Decimal, error) {
	return postgres.RatePeriod(c.ctx, c.tx, tenant, service, meter, start)
}
