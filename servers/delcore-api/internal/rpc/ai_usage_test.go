// SPDX-License-Identifier: FSL-1.1-ALv2
package rpc

import (
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"google.golang.org/protobuf/types/known/timestamppb"
	"testing"
	"time"
)

func TestExactAIRating(t *testing.T) {
	for _, tc := range []struct{ meter, want string }{{"translator", "0.000000345"}, {"extractor", "0.00000039"}} {
		now := timestamppb.New(time.Now())
		u := &pb.Usage{Meter: tc.meter, Quantity: "1", OccurredAt: now, AiUsage: &pb.AiUsage{Model: "openai/gpt-6-luna", PriceEffectiveAt: now, Tokens: []*pb.AiTokenUsage{{Kind: pb.AiTokenKind_AI_TOKEN_KIND_INPUT, Tokens: 3, UsdPerToken: "0.0000001"}}}}
		got, err := rateAI(u)
		if err != nil || got.String() != tc.want {
			t.Fatalf("%s: %s %v", tc.meter, got.String(), err)
		}
		u.AiUsage.Tokens = append(u.AiUsage.Tokens, u.AiUsage.Tokens[0])
		if _, err = rateAI(u); err == nil {
			t.Fatal("duplicate token category accepted")
		}
		u.AiUsage.Tokens = u.AiUsage.Tokens[:1]
		u.AiUsage.Tokens[0].UsdPerToken = "NaN"
		if _, err = rateAI(u); err == nil {
			t.Fatal("invalid price accepted")
		}
	}
}
