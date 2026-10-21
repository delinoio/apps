// SPDX-License-Identifier: Apache-2.0

package clairic

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

const successResponse = `{"success":true,"errors":[],"messages":[],"result":{"model":"clairic","answers":{"id":{"type":"number","value":9007199254740993,"confidence":0,"p_null":0},"missing":{"type":"text","value":null,"guess":"possible"},"contacts":{"type":"list","value":["Dana","Priya"],"spans":[[0,4],[6,11]]},"estimate":{"type":"estimate","value":"2026-11-01","mean":1.5,"median":"2026-11-01","p10":"2026-10-31","p90":"2026-11-02","p_below":0,"p_above":0.01,"quantiles":[1,"2026-11-01"]},"forecast":{"type":"forecast","value":[1,2],"steps":[{"value":1,"median":1},{"value":2,"median":2}]}},"truncated":true,"state_text":"example","usage":{"state_tokens":2,"image_tokens":3,"schema_tokens":4,"input_tokens":9,"output_tokens":0,"passes":2}}}`

func request() Request {
	return Request{State: "example", Fields: map[string]Field{"id": {Type: TypeNumber}}}
}

func testClient(t *testing.T, handler http.HandlerFunc) *Client {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	c, err := NewClient("account", "test-token", &Config{BaseURL: server.URL + "/client/v4", HTTPClient: server.Client()})
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func TestRunWireAndAnswers(t *testing.T) {
	zero, falseValue := 0, false
	c := testClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/client/v4/accounts/account/ai/run/"+Model {
			t.Errorf("unexpected request: %s %s", r.Method, r.URL.Path)
		}
		if r.Header.Get("Authorization") != "Bearer test-token" || r.Header.Get("Content-Type") != "application/json" || r.Header.Get("Accept") != "application/json" {
			t.Error("missing headers")
		}
		var body struct {
			Model         string                                `json:"model"`
			Fields        map[string]map[string]json.RawMessage `json:"fields"`
			Images        []json.RawMessage                     `json:"images"`
			ReferenceDate string                                `json:"reference_date"`
			Options       Options                               `json:"options"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.Model != "clairic" || string(body.Fields["id"]["decimals"]) != "0" || string(body.Fields["id"]["integer"]) != "false" || string(body.Fields["estimate"]["min"]) != "0" {
			t.Errorf("lost default or explicit zero: %+v", body)
		}
		if body.ReferenceDate != "2026-10-21" || !body.Options.IncludeQuantiles || !body.Options.ReturnStateText || len(body.Images) != 2 || body.Images[0][0] != '"' || body.Images[1][0] != '{' {
			t.Errorf("missing optional inputs: %+v", body)
		}
		fmt.Fprint(w, successResponse)
	})
	in := request()
	in.Fields["id"] = Field{Type: TypeNumber, Decimals: &zero, Integer: &falseValue}
	in.Fields["estimate"] = Field{Type: TypeEstimate, Min: 0, Max: 100, Step: 1, Scale: ScaleLinear}
	in.ReferenceDate = "2026-10-21"
	in.Options = &Options{IncludeQuantiles: true, ReturnStateText: true}
	in.Images = []Image{{DataURL: "data:image/png;base64,AQ=="}, {ContentType: "image/webp", Base64: "Ag=="}}
	result, err := c.Run(context.Background(), in)
	if err != nil {
		t.Fatal(err)
	}
	if in.Model != "" {
		t.Error("Run mutated the caller's request")
	}
	var id int64
	if err := result.Answers["id"].DecodeValue(&id); err != nil || id != 9007199254740993 {
		t.Fatalf("lost numeric precision: %d, %v", id, err)
	}
	answer := result.Answers["id"]
	if answer.Confidence == nil || *answer.Confidence != 0 || answer.PNull == nil || *answer.PNull != 0 {
		t.Error("lost zero probabilities")
	}
	var missing *string
	if err := result.Answers["missing"].DecodeValue(&missing); err != nil || missing != nil {
		t.Fatalf("lost null: %v %v", missing, err)
	}
	var contacts []string
	if err := result.Answers["contacts"].DecodeValue(&contacts); err != nil || len(contacts) != 2 || contacts[1] != "Priya" {
		t.Fatalf("invalid list: %v %v", contacts, err)
	}
	estimate := result.Answers["estimate"]
	if string(estimate.Median) != `"2026-11-01"` || len(estimate.Quantiles) != 2 || estimate.PBelow == nil || len(result.Answers["forecast"].Steps) != 2 {
		t.Fatal("lost estimate or forecast metadata")
	}
	if !result.Truncated || result.StateText != "example" || result.Usage.InputTokens != 9 || result.Usage.Passes != 2 {
		t.Fatalf("lost response metadata: %+v", result)
	}
}

func TestRunErrors(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
		api    bool
	}{
		{"rate limit", 429, `{"success":false,"errors":[{"code":1001,"message":"busy"}],"messages":[]}`, true},
		{"API failure", 200, `{"success":false,"errors":[{"code":1001,"message":"busy"}]}`, true},
		{"plain error", 502, "upstream unavailable", true},
		{"invalid JSON", 200, "{", false},
		{"missing success", 200, `{}`, false},
		{"null envelope", 200, `null`, false},
		{"missing result", 200, `{"success":true}`, false},
		{"null result", 200, `{"success":true,"result":null}`, false},
		{"invalid result", 200, `{"success":true,"result":{}}`, false},
		{"wrong result type", 200, `{"success":true,"result":[]}`, false},
		{"trailing data", 200, successResponse + `{}`, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			calls := 0
			c := testClient(t, func(w http.ResponseWriter, r *http.Request) {
				calls++
				w.Header().Set("CF-Ray", "request-id")
				w.WriteHeader(tc.status)
				fmt.Fprint(w, tc.body)
			})
			_, err := c.Run(context.Background(), request())
			if err == nil {
				t.Fatal("expected error")
			}
			var api *APIError
			if errors.As(err, &api) != tc.api {
				t.Fatalf("unexpected error type: %T %v", err, err)
			}
			if api != nil && (api.StatusCode != tc.status || api.RequestID != "request-id") {
				t.Fatalf("lost error metadata: %+v", api)
			}
			if strings.Contains(tc.body, "1001") && (len(api.Errors) != 1 || api.Errors[0].Code != 1001) {
				t.Fatal("lost Cloudflare error details")
			}
			if calls != 1 {
				t.Fatalf("unexpected retry: %d calls", calls)
			}
		})
	}
}

func TestCancellation(t *testing.T) {
	c := testClient(t, func(w http.ResponseWriter, r *http.Request) { t.Error("canceled request reached server") })
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := c.Run(ctx, request())
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation was not preserved: %v", err)
	}
}

func TestValidation(t *testing.T) {
	c := testClient(t, func(w http.ResponseWriter, r *http.Request) { t.Error("invalid request reached server") })
	for _, mutate := range []func(*Request){
		func(r *Request) { r.Model = "other" },
		func(r *Request) { r.State = "" },
		func(r *Request) { r.Fields = nil },
		func(r *Request) { r.Fields["bad name"] = Field{Type: TypeText} },
		func(r *Request) { r.Fields["id"] = Field{Type: "other"} },
		func(r *Request) { r.Fields["id"] = Field{Type: TypeEstimate, Min: make(chan int)} },
		func(r *Request) { r.Images = make([]Image, 5) },
		func(r *Request) { r.Images = []Image{{DataURL: "https://example.com/image.png"}} },
		func(r *Request) { r.Images = []Image{{ContentType: "image/gif", Base64: "AQ=="}} },
		func(r *Request) { r.Images = []Image{{ContentType: "image/png", Base64: "!"}} },
		func(r *Request) { r.Images = []Image{{ContentType: "image/png"}} },
		func(r *Request) { r.Images = []Image{{DataURL: "data:image/png;base64,AQ==", Base64: "AQ=="}} },
		func(r *Request) { r.State = strings.Repeat("a", maxBodySize) },
	} {
		in := request()
		mutate(&in)
		if _, err := c.Run(context.Background(), in); err == nil {
			t.Fatal("expected validation error")
		}
	}
}

func TestImageLimits(t *testing.T) {
	image := Image{ContentType: "image/jpeg", Base64: base64.StdEncoding.EncodeToString(make([]byte, 4<<20))}
	if _, err := image.decodedSize(); err != nil {
		t.Fatal(err)
	}
	in := request()
	in.Model = "clairic"
	in.Images = []Image{image, image}
	if err := validate(in); err != nil {
		t.Fatal(err)
	}
	in.Images = append(in.Images, Image{ContentType: "image/png", Base64: "AQ=="})
	if err := validate(in); err == nil {
		t.Fatal("accepted oversized image total")
	}
	image.Base64 = base64.StdEncoding.EncodeToString(make([]byte, (4<<20)+1))
	if _, err := json.Marshal(image); err == nil {
		t.Fatal("accepted oversized image")
	}
}

func TestImageOnlyInput(t *testing.T) {
	in := request()
	in.State = ""
	in.Model = Model
	in.Images = []Image{{ContentType: "image/jpeg", Base64: "AQ=="}}
	if err := validate(in); err != nil {
		t.Fatal(err)
	}
}

func TestConfig(t *testing.T) {
	for _, tc := range []struct{ account, token, base string }{
		{"", "token", ""}, {"../account", "token", ""}, {"..", "token", ""},
		{"account", "", ""}, {"account", "token\n", ""},
		{"account", "token", "/relative"}, {"account", "token", "ftp://example.com"},
		{"account", "token", "https://user:pass@example.com"}, {"account", "token", "https://example.com?query=1"},
	} {
		if _, err := NewClient(tc.account, tc.token, &Config{BaseURL: tc.base}); err == nil {
			t.Fatal("accepted invalid configuration")
		}
	}
	c, err := NewClient("account", "token", nil)
	if err != nil || c.endpoint != DefaultBaseURL+"/accounts/account/ai/run/"+Model || c.http.Timeout == 0 {
		t.Fatalf("invalid defaults: %+v %v", c, err)
	}
}
