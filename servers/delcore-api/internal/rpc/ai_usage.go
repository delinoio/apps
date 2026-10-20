// SPDX-License-Identifier: FSL-1.1-ALv2
package rpc

import (
	"encoding/json"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"strconv"
)

func (c *call) consumerAuthorization(q *pb.AuthorizeRequest) (bool, error) {
	var name string
	if err := c.tx.QueryRow(c.ctx, "SELECT name FROM services WHERE id=$1", c.service).Scan(&name); err != nil {
		return false, err
	}
	if name != "delitools" {
		return false, forbidden()
	}
	var allowed bool
	switch q.ConsumerAuthorization {
	case pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_TENANT_KEY:
		if q.UserId != "" || !q.Paid {
			return false, invalid("tenant key authorization requires paid execution without user attribution")
		}
		err := c.tx.QueryRow(c.ctx, `SELECT EXISTS(SELECT 1 FROM tenants t JOIN services s ON s.id=$2 WHERE t.id=$1 AND t.deleted_at IS NULL AND $3=ANY(s.capabilities))`, q.TenantId, c.service, q.Capability).Scan(&allowed)
		return allowed, err
	case pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_PERSONAL_KEYS, pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_SERVICE_KEYS:
		if q.Paid || q.Capability != "" {
			return false, invalid("key management is not paid capability execution")
		}
		err := c.tx.QueryRow(c.ctx, `SELECT EXISTS(SELECT 1 FROM members m JOIN users u ON u.id=m.user_id JOIN tenants t ON t.id=m.tenant_id WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND ($3 OR m.roles && ARRAY[1,2]))`, q.TenantId, q.UserId, q.ConsumerAuthorization == pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_PERSONAL_KEYS).Scan(&allowed)
		return allowed, err
	default:
		return false, invalid("unknown consumer authorization")
	}
}

func (c *call) aiCost(u *pb.Usage) (accounting.Decimal, []byte, error) {
	var name string
	if err := c.tx.QueryRow(c.ctx, "SELECT name FROM services WHERE id=$1", c.service).Scan(&name); err != nil {
		return accounting.Zero(), nil, err
	}
	if name != "delitools" {
		if u.AiUsage != nil {
			return accounting.Zero(), nil, forbidden()
		}
		return accounting.Zero(), nil, nil
	}
	cost, err := rateAI(u)
	if err != nil {
		return cost, nil, err
	}
	data, err := json.Marshal(u.AiUsage)
	return cost, data, err
}

// Only the registered reporting service supplies catalog evidence. Delcore owns
// the markup and exact arithmetic; no caller-supplied final fee is accepted.
func rateAI(u *pb.Usage) (accounting.Decimal, error) {
	fail := func() (accounting.Decimal, error) {
		return accounting.Zero(), invalid("invalid measured AI usage or frozen USD catalog price")
	}
	a := u.AiUsage
	if a == nil || len(a.Model) == 0 || len(a.Model) > 200 || a.PriceEffectiveAt == nil || a.PriceEffectiveAt.CheckValid() != nil || u.OccurredAt == nil || a.PriceEffectiveAt.AsTime().After(u.OccurredAt.AsTime()) || len(a.Tokens) == 0 || len(a.Tokens) > 4 || u.Quantity != "1" {
		return fail()
	}
	factor := ""
	switch u.Meter {
	case "translator":
		factor = "1.15"
	case "extractor":
		factor = "1.30"
	default:
		return fail()
	}
	seen := map[pb.AiTokenKind]bool{}
	sum := accounting.Zero()
	for _, token := range a.Tokens {
		if token == nil || token.Kind < pb.AiTokenKind_AI_TOKEN_KIND_INPUT || token.Kind > pb.AiTokenKind_AI_TOKEN_KIND_CACHE_WRITE || seen[token.Kind] || len(token.UsdPerToken) > 64 {
			return fail()
		}
		seen[token.Kind] = true
		price, err := accounting.Parse(token.UsdPerToken)
		if err != nil || price.Cmp(accounting.Zero()) < 0 {
			return fail()
		}
		count, _ := accounting.Parse(strconv.FormatUint(token.Tokens, 10))
		sum = sum.Add(price.Mul(count))
	}
	markup, _ := accounting.Parse(factor)
	return sum.Mul(markup), nil
}

// Corrections may change measured counts, but never the frozen pricing identity.
func sameAIPrices(a, b *pb.AiUsage) bool {
	if a == nil || b == nil {
		return a == b
	}
	if a.Model != b.Model || a.PriceEffectiveAt == nil || b.PriceEffectiveAt == nil || !a.PriceEffectiveAt.AsTime().Equal(b.PriceEffectiveAt.AsTime()) || len(a.Tokens) != len(b.Tokens) {
		return false
	}
	prices := make(map[pb.AiTokenKind]string, len(a.Tokens))
	for _, t := range a.Tokens {
		prices[t.Kind] = t.UsdPerToken
	}
	for _, t := range b.Tokens {
		if price, ok := prices[t.Kind]; !ok || price != t.UsdPerToken {
			return false
		}
	}
	return true
}
