// SPDX-License-Identifier: Apache-2.0

package clairic_test

import (
	"context"
	"log"
	"os"
	"time"

	"github.com/delinoio/apps/go/clairic"
)

func ExampleClient_Run() {
	client, err := clairic.NewClient(os.Getenv("CLOUDFLARE_ACCOUNT_ID"), os.Getenv("CLOUDFLARE_API_TOKEN"), nil)
	if err != nil {
		log.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	response, err := client.Run(ctx, clairic.Request{
		State: "Invoice INV-2291: 4 pallets at $250 each, shipping $80.",
		Fields: map[string]clairic.Field{
			"invoice_number": {Type: clairic.TypeText},
			"total":          {Type: clairic.TypeNumber, Unit: "USD", Instructions: "Amount due including shipping"},
		},
	})
	if err != nil {
		log.Fatal(err)
	}
	var total *float64
	if err := response.Answers["total"].DecodeValue(&total); err != nil {
		log.Fatal(err)
	}
	if total != nil {
		log.Printf("total: %.2f", *total)
	}
}
