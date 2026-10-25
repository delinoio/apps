// SPDX-License-Identifier: Apache-2.0
package cli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/mattn/go-isatty"
	"io"
	"os"
	"sort"
	"strconv"
	"strings"

	"github.com/delinoio/apps/cmds/lane/internal/api"
)

type Options struct {
	Remote, JSON, Once, ConfirmEpochChange, RetryBlocked bool
	Space, ExpectedSpace, Endpoint, RedirectURI, Color   string
	ReauthChallenge                                      string
}
type Invoke func(context.Context, Options, string, json.RawMessage) (any, error)
type App struct {
	Version  string
	In       io.Reader
	Out, Err io.Writer
	Invoke   Invoke
	MCP      func(context.Context, Options, io.Reader, io.Writer) error
}
type command struct{ method, help string }

var commands = map[string]command{
	"repository list": {"ListRepositories", "[--page-size N] [--cursor TOKEN]"}, "repository get": {"GetRepository", "ID"}, "repository create": {"CreateRepository", "--url URL [--display-name NAME]"}, "repository edit": {"SaveRepository", "ID --expected-version UUID --url URL [--display-name NAME]"}, "repository delete": {"DeleteRepository", "ID (permanent)"},
	"task list": {"ListTasks", "[--repository-id UUID] [--progress STATE] [--proposal STATE] [--search TEXT]"}, "task get": {"GetTask", "ID"}, "task create": {"CreateTask", "--repository-id UUID | --repository-url URL; --title TEXT [--description TEXT] [--prompt TEXT | --prompt-stdin] [--progress STATE] [--proposal STATE]"}, "task edit": {"SaveTask", "ID --expected-version UUID [--title TEXT] [--description TEXT] [--prompt TEXT | --prompt-stdin | --clear-prompt]"}, "task move": {"SaveTask", "ID --expected-version UUID --repository-id UUID"}, "task progress": {"SetProgress", "ID --expected-version UUID --progress STATE"}, "task accept": {"SetProposal", "ID --expected-version UUID"}, "task reject": {"SetProposal", "ID --expected-version UUID"}, "task delete": {"DeleteTask", "ID (permanent)"}, "task history": {"ListHistory", "ID [--page-size N] [--cursor TOKEN]"}, "task prompt-history": {"ListHistory", "ID [--page-size N] [--cursor TOKEN]"}, "task restore-prompt": {"RestorePrompt", "ID --expected-version UUID --source-version UUID"},
	"conflict list": {"ConflictList", "[--kind task|repository]"}, "conflict get": {"ConflictGet", "ID --kind task|repository"}, "conflict resolve": {"ResolveConflict", "ID --expected-head UUID (repeat for all heads) --resolved-json JSON"},
	"sync cancel-reauth": {"CancelReauthentication", "--challenge-id ID (cancel pending browser reauthentication)"},
	"sync status":        {"GetLocalStatus", "(local)"}, "sync connect": {"ConnectSync", "[--redirect-uri URI] [--endpoint URL] [--account-id ID] [--confirm-epoch-change] (browser approval)"}, "sync callback": {"SyncCallback", "CALLBACK_URI (validates browser state)"}, "sync run": {"SyncRun", "[--once] [--retry-blocked] [--endpoint URL] (local durable worker)"}, "sync disconnect": {"DisconnectSync", "--retention keep-local|remove-local"},
	"notification get": {"GetNotificationPreferences", "(per-device)"}, "notification set": {"SetNotificationPreferences", "--enabled true|false [--content detail|generic]"},
	"device list": {"ListDevices", "--remote"}, "device disconnect": {"DisconnectDevice", "--remote --device-id UUID"}, "connection list": {"ListConnections", "--remote"}, "connection revoke": {"RevokeConnection", "--remote --connection-id UUID"},
	"receiver list": {"ListReceivers", "--remote [--repository-id UUID]"}, "receiver create": {"CreateReceiver", "--remote --repository-id UUID --name NAME"}, "receiver edit": {"SaveReceiver", "--remote ID --expected-revision N --repository-id UUID --name NAME"}, "receiver rotate-key": {"RotateReceiverKey", "--remote ID --expected-revision N"}, "receiver delete": {"DeleteReceiver", "--remote ID (retains events)"},
	"event list": {"ListEvents", "[--repository-id UUID] [--states STATE] (local cached read or --remote)"}, "event get": {"GetEvent", "ID (local cached read or --remote)"}, "event delete": {"DeleteEvent", "--remote ID (permanent)"}, "event claim": {"ClaimEvent", "--remote ID"}, "event renew": {"RenewEventLease", "--remote ID --lease-token TOKEN"}, "event release": {"ReleaseEventLease", "--remote ID --lease-token TOKEN [--error-reason TEXT]"}, "event complete": {"CompleteEvent", "--remote ID --lease-token TOKEN --tasks-json JSON_ARRAY"}, "event reopen": {"ReopenEvent", "--remote ID --expected-revision N"},
	"data remove-local": {"RemoveDeviceData", "(selected local space, permanent)"}, "data delete-everywhere": {"DeleteEverywhere", "--remote --confirmed-account-id ID (browser reauthentication)"}, "data clear-server": {"ClearServer", "--remote --confirmed-account-id ID (browser reauthentication)"},
	"login": {"ConnectSync", "[--redirect-uri URI] [--endpoint URL]"}, "logout": {"DisconnectSync", "--retention keep-local|remove-local"}, "mcp serve": {"MCP", "(stdio; --remote explicitly selects online tools)"}, "rpc": {"RPC", "METHOD --request-json JSON|- (typed helper interface)"},
}

func (a App) Run(ctx context.Context, args []string) int {
	opt := Options{}
	for _, x := range args {
		if x == "--json" {
			opt.JSON = true
		}
	}
	code, err := a.run(ctx, args, &opt)
	if err == nil {
		return code
	}
	f := api.Error(err)
	if code == 2 {
		f.Reason = "ERROR_REASON_INVALID_INPUT"
		f.Message = err.Error()
	}
	if opt.JSON {
		_ = json.NewEncoder(a.Out).Encode(map[string]any{"ok": false, "error": f})
	} else {
		fmt.Fprintln(a.Err, f.Message)
	}
	return code
}
func (a App) run(ctx context.Context, args []string, opt *Options) (int, error) {
	flags := map[string][]string{}
	var words []string
	bools := map[string]bool{"json": true, "remote": true, "once": true, "help": true, "version": true, "prompt-stdin": true, "clear-prompt": true, "confirm-epoch-change": true, "retry-blocked": true}
	for i := 0; i < len(args); i++ {
		x := args[i]
		if x == "-h" {
			x = "--help"
		}
		if !strings.HasPrefix(x, "--") {
			words = append(words, x)
			continue
		}
		k, v, has := strings.Cut(x[2:], "=")
		if !has {
			if bools[k] {
				v = "true"
			} else {
				i++
				if i >= len(args) {
					return 2, fmt.Errorf("--%s requires a value", k)
				}
				v = args[i]
			}
		}
		flags[k] = append(flags[k], v)
	}
	pop := func(k string) string {
		v := flags[k]
		delete(flags, k)
		if len(v) == 0 {
			return ""
		}
		return v[len(v)-1]
	}
	opt.JSON = pop("json") == "true"
	opt.Remote = pop("remote") == "true"
	opt.Once = pop("once") == "true"
	opt.ConfirmEpochChange = pop("confirm-epoch-change") == "true"
	opt.RetryBlocked = pop("retry-blocked") == "true"
	opt.Space = pop("space")
	opt.ExpectedSpace = pop("expected-space")
	opt.ReauthChallenge = pop("reauth-challenge")
	opt.Endpoint = pop("endpoint")
	opt.RedirectURI = pop("redirect-uri")
	opt.Color = pop("color")
	if pop("version") == "true" {
		fmt.Fprintln(a.Out, "lane "+a.Version)
		return 0, nil
	}
	help := pop("help") == "true"
	if help || len(words) == 0 {
		a.help()
		return 0, nil
	}
	if opt.Remote && opt.Space != "" {
		return 2, errors.New("--space cannot be used with --remote")
	}
	if !opt.Remote && opt.ExpectedSpace != "" {
		return 2, errors.New("--expected-space requires --remote")
	}
	if opt.Color != "" && opt.Color != "never" && opt.Color != "auto" {
		return 2, errors.New("--color must be auto or never")
	}
	key := words[0]
	n := 1
	if _, ok := commands[key]; !ok && len(words) > 1 {
		key += " " + words[1]
		n = 2
	}
	cmd, ok := commands[key]
	if !ok {
		return 2, errors.New("unknown command; use --help")
	}
	pos := words[n:]
	if opt.ConfirmEpochChange && cmd.method != "ConnectSync" {
		return 2, errors.New("--confirm-epoch-change requires sync connect or login")
	}
	if opt.RetryBlocked && cmd.method != "SyncRun" {
		return 2, errors.New("--retry-blocked requires sync run")
	}
	if cmd.method == "MCP" {
		if len(pos) != 0 || len(flags) != 0 {
			return 2, errors.New("unexpected MCP argument")
		}
		if a.MCP == nil {
			return 1, errors.New("MCP unavailable")
		}
		if err := a.MCP(ctx, *opt, a.In, a.Out); err != nil {
			return 1, err
		}
		return 0, nil
	}
	raw := pop("request-json")
	body := map[string]any{}
	if raw != "" {
		if raw == "-" {
			b, e := io.ReadAll(a.In)
			if e != nil {
				return 1, e
			}
			raw = string(b)
		}
		if err := json.Unmarshal([]byte(raw), &body); err != nil || body == nil {
			return 2, errors.New("--request-json requires a JSON object")
		}
	}
	method := cmd.method
	if method == "RPC" {
		if len(pos) != 1 {
			return 2, errors.New("rpc requires an exact typed method name")
		}
		method = pos[0]
		pos = nil
		if api.Descriptor(method) == nil {
			return 2, errors.New("unknown typed method")
		}
	}
	if opt.ReauthChallenge != "" && (!opt.Remote || method != "DeleteAccountData") {
		return 2, errors.New("--reauth-challenge requires remote DeleteAccountData")
	}
	if len(pos) > 1 {
		return 2, errors.New("too many positional arguments")
	}
	if len(pos) == 1 {
		if method == "SyncCallback" {
			body["callbackUri"] = pos[0]
		} else {
			body["id"] = pos[0]
		}
	}
	// Explicit flags override fields in the optional typed request object.
	mapping := map[string]string{"id": "id", "url": "url", "display-name": "displayName", "title": "title", "description": "description", "prompt": "prompt", "repository-id": "repositoryId", "repository-url": "repositoryUrl", "expected-version": "expectedVersionId", "source-version": "sourceVersionId", "search": "search", "device-id": "deviceId", "connection-id": "connectionId", "name": "name", "lease-token": "leaseToken", "error-reason": "errorReason", "expected-revision": "expectedRevision", "confirmed-account-id": "confirmedAccountId", "reauth-proof": "reauthProof", "account-id": "accountId", "kind": "kind", "reauth-callback": "reauthCallback", "challenge-id": "challengeId"}
	for k, f := range mapping {
		if v, ok := flags[k]; ok {
			body[f] = v[len(v)-1]
			delete(flags, k)
		}
	}
	for _, pair := range [][2]string{{"progress", "PROGRESS_"}, {"proposal", "PROPOSAL_"}, {"states", "EVENT_STATE_"}} {
		if values, ok := flags[pair[0]]; ok {
			var out []string
			for _, v := range values {
				for _, p := range strings.Split(v, ",") {
					p = strings.ReplaceAll(strings.ToUpper(p), "-", "_")
					if !strings.HasPrefix(p, pair[1]) {
						p = pair[1] + p
					}
					out = append(out, p)
				}
			}
			if method == "ListTasks" || method == "ListEvents" {
				body[pair[0]] = out
			} else if len(out) == 1 {
				body[pair[0]] = out[0]
			} else {
				return 2, errors.New("mutation requires one state")
			}
			delete(flags, pair[0])
		}
	}
	if v, ok := flags["expected-head"]; ok {
		body["expectedHeadIds"] = v
		delete(flags, "expected-head")
	}
	for k, f := range map[string]string{"tasks-json": "tasks", "resolved-json": "resolved"} {
		if s := pop(k); s != "" {
			var v any
			if err := json.Unmarshal([]byte(s), &v); err != nil {
				return 2, fmt.Errorf("invalid --%s", k)
			}
			body[f] = v
		}
	}
	page := map[string]any{}
	if s := pop("page-size"); s != "" {
		v, e := strconv.ParseUint(s, 10, 32)
		if e != nil {
			return 2, errors.New("invalid page size")
		}
		page["size"] = v
	}
	if s := pop("cursor"); s != "" {
		page["cursor"] = s
	}
	if len(page) > 0 {
		body["page"] = page
	}
	if s := pop("request-id"); s != "" {
		body["meta"] = map[string]any{"requestId": s}
	}
	if s := pop("enabled"); s != "" {
		b, e := strconv.ParseBool(s)
		if e != nil {
			return 2, errors.New("--enabled requires true or false")
		}
		body["enabled"] = b
	}
	if s := pop("content"); s != "" {
		body["content"] = "NOTIFICATION_CONTENT_" + strings.ToUpper(s)
	}
	if s := pop("retention"); s != "" {
		if s != "keep-local" && s != "remove-local" {
			return 2, errors.New("--retention requires keep-local or remove-local")
		}
		body["retention"] = "LOCAL_RETENTION_" + strings.ReplaceAll(strings.ToUpper(s), "-", "_")
	}
	stdin := pop("prompt-stdin") == "true"
	clear := pop("clear-prompt") == "true"
	if stdin {
		if _, ok := body["prompt"]; ok || clear {
			return 2, errors.New("choose one prompt source")
		}
		b, e := io.ReadAll(a.In)
		if e != nil {
			return 1, e
		}
		body["prompt"] = string(b)
	}
	if clear {
		body["prompt"] = nil
	}
	if len(flags) > 0 {
		keys := make([]string, 0, len(flags))
		for k := range flags {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		return 2, fmt.Errorf("unknown flag --%s", keys[0])
	}
	if key == "task accept" {
		body["proposal"] = "PROPOSAL_ACCEPTED"
	}
	if key == "task reject" {
		body["proposal"] = "PROPOSAL_REJECTED"
	}
	if key == "task prompt-history" {
		body["promptsOnly"] = true
	}
	if method == "CreateTask" {
		target := map[string]any{}
		if v, ok := body["repositoryId"]; ok {
			target["id"] = v
			delete(body, "repositoryId")
		}
		if v, ok := body["repositoryUrl"]; ok {
			target["url"] = v
			delete(body, "repositoryUrl")
		}
		if len(target) > 0 {
			body["repository"] = target
		}
	}
	if method == "SaveTask" {
		if _, ok := body["task"]; !ok {
			if body["expectedVersionId"] == nil {
				return 2, errors.New("--expected-version is required")
			}
			q, _ := json.Marshal(map[string]any{"id": body["id"]})
			r, e := a.Invoke(ctx, *opt, "GetTask", q)
			if e != nil {
				return 1, e
			}
			b, e := api.JSON(r)
			if e != nil {
				return 1, e
			}
			var read struct {
				Result struct {
					Version struct {
						Task map[string]any `json:"task"`
					} `json:"version"`
				} `json:"result"`
			}
			if json.Unmarshal(b, &read) != nil || read.Result.Version.Task == nil {
				return 1, errors.New("task unavailable")
			}
			task := read.Result.Version.Task
			for _, f := range []string{"title", "description", "prompt", "repositoryId", "progress", "proposal"} {
				if v, ok := body[f]; ok {
					if v == nil {
						delete(task, f)
					} else {
						task[f] = v
					}
					delete(body, f)
				}
			}
			body["task"] = task
		}
	}

	required := map[string][]string{
		"GetRepository": {"id"}, "CreateRepository": {"url"}, "SaveRepository": {"id", "expectedVersionId", "url"}, "DeleteRepository": {"id"},
		"GetTask": {"id"}, "CreateTask": {"repository", "title"}, "SaveTask": {"id", "expectedVersionId", "task"}, "SetProgress": {"id", "expectedVersionId", "progress"}, "SetProposal": {"id", "expectedVersionId", "proposal"}, "RestorePrompt": {"id", "expectedVersionId", "sourceVersionId"}, "DeleteTask": {"id"}, "ListHistory": {"id"}, "ResolveConflict": {"id", "expectedHeadIds", "resolved"},
		"CancelReauthentication": {"challengeId"}, "DisconnectSync": {"retention"}, "SyncCallback": {"callbackUri"}, "SetNotificationPreferences": {"enabled"}, "DisconnectDevice": {"deviceId"}, "RevokeConnection": {"connectionId"},
		"CreateReceiver": {"repositoryId", "name"}, "SaveReceiver": {"id", "expectedRevision", "repositoryId", "name"}, "RotateReceiverKey": {"id", "expectedRevision"}, "DeleteReceiver": {"id"}, "GetEvent": {"id"}, "DeleteEvent": {"id"}, "ClaimEvent": {"id"}, "RenewEventLease": {"id", "leaseToken"}, "ReleaseEventLease": {"id", "leaseToken"}, "CompleteEvent": {"id", "leaseToken"}, "ReopenEvent": {"id", "expectedRevision"}, "ConflictGet": {"id"},
	}
	for _, field := range required[method] {
		if value, exists := body[field]; !exists || value == nil || value == "" {
			return 2, fmt.Errorf("%s requires %s; use --help", key, field)
		}
	}
	if d := api.Descriptor(method); d != nil {
		b, _ := json.Marshal(body)
		if _, e := api.Decode(method, b); e != nil {
			return 2, e
		}
	}
	b, e := json.Marshal(body)
	if e != nil {
		return 2, e
	}
	v, e := a.Invoke(ctx, *opt, method, b)
	if e != nil {
		return 1, e
	}
	return 0, a.output(v, *opt)
}
func (a App) output(v any, opt Options) error {
	b, e := api.JSON(v)
	if e != nil {
		return e
	}
	if opt.JSON {
		_, e = fmt.Fprintf(a.Out, "{\"ok\":true,\"result\":%s}\n", b)
		return e
	}
	var pretty any
	if e = json.Unmarshal(b, &pretty); e != nil {
		return e
	}
	if f, ok := a.Out.(*os.File); ok && opt.Color != "never" && os.Getenv("NO_COLOR") == "" && isatty.IsTerminal(f.Fd()) {
		fmt.Fprint(a.Out, "\x1b[36m")
		defer fmt.Fprint(a.Out, "\x1b[0m")
	}
	return human(a.Out, pretty, "")
}
func human(w io.Writer, v any, indent string) error {
	switch x := v.(type) {
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			switch val := x[k].(type) {
			case map[string]any, []any:
				fmt.Fprintf(w, "%s%s:\n", indent, k)
				if e := human(w, val, indent+"  "); e != nil {
					return e
				}
			default:
				if _, e := fmt.Fprintf(w, "%s%s: %v\n", indent, k, val); e != nil {
					return e
				}
			}
		}
	case []any:
		for _, v := range x {
			fmt.Fprintln(w, indent+"-")
			if e := human(w, v, indent+"  "); e != nil {
				return e
			}
		}
	default:
		_, e := fmt.Fprintf(w, "%s%v\n", indent, x)
		return e
	}
	return nil
}
func (a App) help() {
	fmt.Fprintln(a.Out, "Lane — local task and prompt management\nUsage: lane [--remote] [--json] [--space UUID] COMMAND\nLocal commands need no login or running app. --remote explicitly selects server data.\nMutations accept --request-id UUID; updates require --expected-version UUID.\nAll typed commands accept --request-json JSON or - for stdin.\n--expected-space UUID with --remote refuses a changed active account.\n--color=never or NO_COLOR disables terminal color.")
	keys := make([]string, 0, len(commands))
	for k := range commands {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		fmt.Fprintf(a.Out, "  %-24s %s\n", k, commands[k].help)
	}
}
