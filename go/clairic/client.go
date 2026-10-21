// SPDX-License-Identifier: Apache-2.0

// Package clairic calls Cloudflare Workers AI Clairic using the REST API.
package clairic

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"
)

const DefaultBaseURL = "https://api.cloudflare.com/client/v4"
const maxBodySize = 16 << 20

// Config customizes HTTP behavior. BaseURL includes the API prefix and is
// trusted to receive the token. A nil HTTPClient uses a two-minute timeout.
type Config struct {
	BaseURL    string
	HTTPClient *http.Client
}

// Client is safe for concurrent calls when its HTTP client is safe for them.
type Client struct {
	endpoint string
	token    string
	http     *http.Client
}

// NewClient creates a client without making a network request.
func NewClient(accountID, token string, config *Config) (*Client, error) {
	if strings.TrimSpace(accountID) == "" || strings.ContainsAny(accountID, "/?#%\\") || accountID == "." || accountID == ".." {
		return nil, fmt.Errorf("clairic: account ID must be a non-empty path segment")
	}
	if strings.TrimSpace(token) == "" || strings.ContainsAny(token, "\r\n") {
		return nil, fmt.Errorf("clairic: invalid API token")
	}
	base := DefaultBaseURL
	hc := &http.Client{Timeout: 2 * time.Minute}
	if config != nil {
		if config.BaseURL != "" {
			base = config.BaseURL
		}
		if config.HTTPClient != nil {
			hc = config.HTTPClient
		}
	}
	u, err := url.Parse(base)
	if err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return nil, fmt.Errorf("clairic: base URL must be an absolute HTTP(S) URL without credentials, query or fragment")
	}
	return &Client{endpoint: strings.TrimRight(u.String(), "/") + "/accounts/" + url.PathEscape(accountID) + "/ai/run/" + Model, token: token, http: hc}, nil
}

// Message is an error or informational message from the Cloudflare envelope.
type Message struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// APIError represents an HTTP failure or unsuccessful Cloudflare envelope.
// It deliberately excludes raw response bodies from its error string.
type APIError struct {
	StatusCode int
	Errors     []Message
	Messages   []Message
	RequestID  string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("clairic: API request failed (HTTP %d, request ID %q)", e.StatusCode, e.RequestID)
}

// Run executes one request without automatic retries. Field-specific semantic
// validation is performed by Clairic; basic shape and image limits are local.
func (c *Client) Run(ctx context.Context, input Request) (*Response, error) {
	if input.Model == "" {
		input.Model = "clairic"
	}
	if err := validate(input); err != nil {
		return nil, err
	}
	body, err := json.Marshal(input)
	if err != nil {
		return nil, fmt.Errorf("clairic: encode request: %w", err)
	}
	if len(body) > maxBodySize {
		return nil, fmt.Errorf("clairic: request exceeds 16 MiB")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.endpoint, bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("clairic: create request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("clairic: send request: %w", err)
	}
	defer resp.Body.Close()
	apiErr := &APIError{StatusCode: resp.StatusCode, RequestID: resp.Header.Get("CF-Ray")}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxBodySize+1))
	if err != nil {
		return nil, fmt.Errorf("clairic: read response: %w", err)
	}
	httpOK := resp.StatusCode >= 200 && resp.StatusCode < 300
	if len(data) > maxBodySize {
		if !httpOK {
			return nil, apiErr
		}
		return nil, fmt.Errorf("clairic: response exceeds 16 MiB")
	}
	var envelope struct {
		Result   json.RawMessage `json:"result"`
		Success  *bool           `json:"success"`
		Errors   []Message       `json:"errors"`
		Messages []Message       `json:"messages"`
	}
	err = json.Unmarshal(data, &envelope)
	apiErr.Errors, apiErr.Messages = envelope.Errors, envelope.Messages
	if !httpOK {
		return nil, apiErr
	}
	if err != nil {
		return nil, fmt.Errorf("clairic: decode envelope: %w", err)
	}
	if envelope.Success == nil {
		return nil, fmt.Errorf("clairic: response is missing success")
	}
	if !*envelope.Success {
		return nil, apiErr
	}
	var result *Response
	if err := json.Unmarshal(envelope.Result, &result); err != nil {
		return nil, fmt.Errorf("clairic: decode result: %w", err)
	}
	if result == nil || result.Model == "" || result.Answers == nil {
		return nil, fmt.Errorf("clairic: response is missing model result")
	}
	return result, nil
}

var fieldName = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_.\-]{0,63}$`)

func validate(r Request) error {
	if r.Model != "clairic" && r.Model != Model {
		return fmt.Errorf("clairic: unsupported model")
	}
	if len(r.Fields) == 0 {
		return fmt.Errorf("clairic: at least one field is required")
	}
	if r.State == "" && len(r.Images) == 0 {
		return fmt.Errorf("clairic: state or images are required")
	}
	for name, field := range r.Fields {
		if !fieldName.MatchString(name) {
			return fmt.Errorf("clairic: invalid field name %q", name)
		}
		switch field.Type {
		case TypeText, TypeNumber, TypeDate, TypeTime, TypeDatetime, TypeDuration, TypeList, TypeEstimate, TypeForecast:
		default:
			return fmt.Errorf("clairic: invalid field type for %q", name)
		}
	}
	if len(r.Images) > 4 {
		return fmt.Errorf("clairic: at most four images are allowed")
	}
	total := 0
	for _, image := range r.Images {
		size, err := image.decodedSize()
		if err != nil {
			return err
		}
		total += size
	}
	if total > 8<<20 {
		return fmt.Errorf("clairic: images exceed 8 MiB total")
	}
	return nil
}
