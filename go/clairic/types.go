// SPDX-License-Identifier: Apache-2.0

package clairic

import "encoding/json"

// Model is the hosted Workers AI model identifier.
const Model = "@cf/cloudflare/clairic"

// FieldType selects the kind of answer Clairic produces.
type FieldType string

const (
	TypeText     FieldType = "text"
	TypeNumber   FieldType = "number"
	TypeDate     FieldType = "date"
	TypeTime     FieldType = "time"
	TypeDatetime FieldType = "datetime"
	TypeDuration FieldType = "duration"
	TypeList     FieldType = "list"
	TypeEstimate FieldType = "estimate"
	TypeForecast FieldType = "forecast"
)

// ItemType selects the element type of a list field.
type ItemType string

const (
	ItemText   ItemType = "text"
	ItemNumber ItemType = "number"
	ItemDate   ItemType = "date"
)

// Scale selects the estimate grid scale.
type Scale string

const (
	ScaleLinear Scale = "linear"
	ScaleLog    Scale = "log"
)

// Interval selects the time between forecast steps.
type Interval string

const (
	IntervalStep    Interval = "step"
	IntervalMinute  Interval = "minute"
	IntervalHour    Interval = "hour"
	IntervalDay     Interval = "day"
	IntervalWeek    Interval = "week"
	IntervalMonth   Interval = "month"
	IntervalQuarter Interval = "quarter"
	IntervalYear    Interval = "year"
)

// Field describes one requested answer. Clairic validates field-specific rules.
type Field struct {
	Type         FieldType `json:"type"`
	Instructions string    `json:"instructions,omitempty"`
	Unit         string    `json:"unit,omitempty"`
	Integer      *bool     `json:"integer,omitempty"`
	Decimals     *int      `json:"decimals,omitempty"`
	Items        ItemType  `json:"items,omitempty"`
	// Min and Max accept numbers or ISO date strings. Step accepts a positive
	// number (days for dates), "month", or "year".
	Min           any      `json:"min,omitempty"`
	Max           any      `json:"max,omitempty"`
	Step          any      `json:"step,omitempty"`
	Scale         Scale    `json:"scale,omitempty"`
	NullThreshold *float64 `json:"null_threshold,omitempty"`
	Horizon       *int     `json:"horizon,omitempty"`
	Interval      Interval `json:"interval,omitempty"`
}

// Options controls optional model output.
type Options struct {
	IncludeQuantiles bool `json:"include_quantiles,omitempty"`
	ReturnStateText  bool `json:"return_state_text,omitempty"`
}

// Request is the Clairic input. An empty Model defaults to "clairic".
type Request struct {
	Model         string           `json:"model"`
	State         string           `json:"state,omitempty"`
	Fields        map[string]Field `json:"fields"`
	Images        []Image          `json:"images,omitempty"`
	ReferenceDate string           `json:"reference_date,omitempty"`
	Options       *Options         `json:"options,omitempty"`
}

// Response is the model result inside the Cloudflare REST envelope.
type Response struct {
	Model     string            `json:"model"`
	Answers   map[string]Answer `json:"answers"`
	Truncated bool              `json:"truncated"`
	StateText string            `json:"state_text,omitempty"`
	Usage     Usage             `json:"usage"`
}

// Answer preserves heterogeneous values without converting numbers to float64.
// RawMessage fields retain JSON null and can be decoded into application types.
type Answer struct {
	Type       FieldType         `json:"type"`
	Value      json.RawMessage   `json:"value"`
	Guess      json.RawMessage   `json:"guess,omitempty"`
	PNull      *float64          `json:"p_null,omitempty"`
	Confidence *float64          `json:"confidence,omitempty"`
	Span       []int             `json:"span,omitempty"`
	Spans      [][]int           `json:"spans,omitempty"`
	Mean       json.RawMessage   `json:"mean,omitempty"`
	Median     json.RawMessage   `json:"median,omitempty"`
	P10        json.RawMessage   `json:"p10,omitempty"`
	P90        json.RawMessage   `json:"p90,omitempty"`
	PBelow     *float64          `json:"p_below,omitempty"`
	PAbove     *float64          `json:"p_above,omitempty"`
	Quantiles  []json.RawMessage `json:"quantiles,omitempty"`
	// The upstream schema leaves the forecast step shape unspecified.
	Steps []json.RawMessage `json:"steps,omitempty"`
}

// DecodeValue decodes Value into dst, which must be a non-nil pointer.
// Use a pointer-to-pointer or json.RawMessage destination to retain JSON null.
func (a Answer) DecodeValue(dst any) error { return json.Unmarshal(a.Value, dst) }

// Usage reports token consumption and model passes.
type Usage struct {
	StateTokens  int `json:"state_tokens"`
	ImageTokens  int `json:"image_tokens"`
	SchemaTokens int `json:"schema_tokens"`
	InputTokens  int `json:"input_tokens"`
	OutputTokens int `json:"output_tokens"`
	Passes       int `json:"passes"`
}
