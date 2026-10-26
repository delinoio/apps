// SPDX-License-Identifier: Apache-2.0
package main

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
	"go.yaml.in/yaml/v4"
)

type bump string

const (
	patch bump = "patch"
	minor bump = "minor"
	major bump = "major"
	none  bump = "none"
)

type record struct {
	Packages map[string]bump `json:"packages"`
	Summary  string          `json:"summary"`
	Public   string          `json:"public,omitempty"`
}
type pending struct {
	ID string `json:"id"`
	record
}

var validID = regexp.MustCompile(`^[a-z0-9]+(?:-[a-z0-9]+)*$`)

func main() {
	if err := run(".", os.Args[1:], os.Stdout); err != nil {
		if os.Getenv("NO_COLOR") == "" {
			fmt.Fprintf(os.Stderr, "\x1b[31mchangeset: %v\x1b[0m\n", err)
		} else {
			fmt.Fprintf(os.Stderr, "changeset: %v\n", err)
		}
		os.Exit(1)
	}
}

func inventory(root string) (map[string]string, error) {
	data, err := os.ReadFile(filepath.Join(root, "pnpm-workspace.yaml"))
	if err != nil {
		return nil, err
	}
	var workspace struct {
		Packages []string `yaml:"packages"`
	}
	if err = yaml.Unmarshal(data, &workspace); err != nil {
		return nil, err
	}
	paths := map[string]bool{"package.json": true}
	for _, pattern := range workspace.Packages {
		if strings.HasPrefix(pattern, "!") {
			continue
		}
		if filepath.IsAbs(pattern) || strings.Contains(pattern, "..") {
			return nil, fmt.Errorf("workspace pattern must stay inside repository: %q", pattern)
		}
		matches, err := doublestar.FilepathGlob(filepath.Join(root, pattern, "package.json"), doublestar.WithFailOnIOErrors())
		if err != nil {
			return nil, err
		}
		for _, match := range matches {
			rel, err := filepath.Rel(root, match)
			if err != nil {
				return nil, err
			}
			// pnpm excludes dependency and VCS directories even for recursive globs.
			ignored := false
			for _, part := range strings.Split(filepath.ToSlash(rel), "/") {
				if part == "node_modules" || part == ".git" || part == "bower_components" {
					ignored = true
				}
			}
			if !ignored {
				paths[filepath.ToSlash(rel)] = true
			}
		}
	}
	result := map[string]string{}
	for _, path := range sortedKeys(paths) {
		excluded := false
		for _, pattern := range workspace.Packages {
			if !strings.HasPrefix(pattern, "!") || path == "package.json" {
				continue
			}
			match, err := doublestar.Match(strings.TrimPrefix(pattern, "!"), filepath.ToSlash(filepath.Dir(path)))
			if err != nil {
				return nil, err
			}
			excluded = excluded || match
		}
		if excluded {
			continue
		}
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			return nil, err
		}
		var pkg struct {
			Name string `json:"name"`
		}
		if err = json.Unmarshal(data, &pkg); err != nil {
			return nil, fmt.Errorf("%s: %w", path, err)
		}
		if strings.TrimSpace(pkg.Name) == "" {
			return nil, fmt.Errorf("%s: package name is required", path)
		}
		if previous, exists := result[pkg.Name]; exists {
			return nil, fmt.Errorf("duplicate package %q in %s and %s", pkg.Name, previous, path)
		}
		result[pkg.Name] = path
	}
	return result, nil
}
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for key := range m {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

const publicStart = "<!-- public-changelog:start -->"
const publicEnd = "<!-- public-changelog:end -->"

func decode(data []byte, r *record) error {
	contents := strings.TrimSpace(strings.ReplaceAll(string(data), "\r\n", "\n"))
	lines := strings.Split(contents, "\n")
	if len(lines) < 2 || strings.TrimSpace(lines[0]) != "---" {
		return errors.New("changeset must start with YAML frontmatter delimited by ---")
	}
	end := 1
	for end < len(lines) && strings.TrimSpace(lines[end]) != "---" {
		end++
	}
	if end == len(lines) {
		return errors.New("missing closing frontmatter delimiter")
	}
	var document yaml.Node
	if err := yaml.Unmarshal([]byte(strings.Join(lines[1:end], "\n")), &document); err != nil {
		return err
	}
	r.Packages = map[string]bump{}
	if len(document.Content) > 0 {
		mapping := document.Content[0]
		// Empty frontmatter is the standard empty changeset form.
		if mapping.Kind != yaml.MappingNode && !(mapping.Kind == yaml.ScalarNode && mapping.Tag == "!!null" && mapping.Value == "") {
			return errors.New("frontmatter must map package names to release types")
		}
		for i := 0; i < len(mapping.Content); i += 2 {
			key, value := mapping.Content[i], mapping.Content[i+1]
			if key.Kind != yaml.ScalarNode || key.Tag != "!!str" || strings.TrimSpace(key.Value) == "" || value.Kind != yaml.ScalarNode || value.Tag != "!!str" {
				return errors.New("package names and release types must be strings")
			}
			if _, exists := r.Packages[key.Value]; exists {
				return fmt.Errorf("duplicate package %q", key.Value)
			}
			r.Packages[key.Value] = bump(value.Value)
		}
	}
	body := strings.TrimSpace(strings.Join(lines[end+1:], "\n"))
	r.Summary, r.Public = body, ""
	if !strings.Contains(body, publicStart) && !strings.Contains(body, publicEnd) {
		return nil
	}
	if strings.Count(body, publicStart) != 1 || strings.Count(body, publicEnd) != 1 {
		return errors.New("public changelog must have exactly one start and end marker")
	}
	before, rest, _ := strings.Cut(body, publicStart)
	note, after, found := strings.Cut(rest, publicEnd)
	if !found || strings.TrimSpace(after) != "" || (before != "" && !strings.HasSuffix(before, "\n")) || !strings.HasPrefix(note, "\n") || !strings.HasSuffix(note, "\n") {
		return errors.New("public changelog markers must occupy separate lines at the end of the body")
	}
	// Reject markers in code fences: treating example code as public prose could
	// expose internal notes. Authors must keep this reserved block outside code.
	fence := ""
	for _, line := range strings.Split(before, "\n") {
		trimmed := strings.TrimLeft(line, " \t")
		if fence == "" {
			for _, prefix := range []string{"```", "~~~"} {
				if strings.HasPrefix(trimmed, prefix) {
					n := 0
					for n < len(trimmed) && trimmed[n] == prefix[0] {
						n++
					}
					fence = trimmed[:n]
					break
				}
			}
		} else if strings.HasPrefix(trimmed, fence) && strings.Trim(strings.TrimSpace(trimmed), fence[:1]) == "" {
			fence = ""
		}
	}
	if fence != "" {
		return errors.New("public changelog block must not be inside a code fence")
	}
	r.Summary, r.Public = strings.TrimSpace(before), strings.TrimSpace(note)
	return nil
}
func validate(r record, packages map[string]string) error {
	for _, name := range sortedKeys(r.Packages) {
		if _, ok := packages[name]; !ok {
			return fmt.Errorf("unknown package %q", name)
		}
		switch r.Packages[name] {
		case patch, minor, major, none:
		default:
			return fmt.Errorf("invalid bump %q for %s", r.Packages[name], name)
		}
	}
	return nil
}
func readPending(root string, packages map[string]string) ([]pending, error) {
	entries, err := os.ReadDir(filepath.Join(root, ".changeset"))
	if err != nil {
		return nil, err
	}
	result := make([]pending, 0, len(entries))
	for _, entry := range entries {
		if (strings.EqualFold(entry.Name(), "README.md") || entry.Name() == "config.json") && entry.Type().IsRegular() {
			continue
		}
		id := strings.TrimSuffix(entry.Name(), ".md")
		if !entry.Type().IsRegular() || !strings.HasSuffix(entry.Name(), ".md") || !validID.MatchString(id) {
			return nil, fmt.Errorf("unexpected changeset entry %q", entry.Name())
		}
		data, err := os.ReadFile(filepath.Join(root, ".changeset", entry.Name()))
		if err != nil {
			return nil, err
		}
		var r record
		if err = decode(data, &r); err == nil {
			err = validate(r, packages)
		}
		if err != nil {
			return nil, fmt.Errorf("%s: %w", entry.Name(), err)
		}
		result = append(result, pending{ID: id, record: r})
	}
	return result, nil
}

type packageFlags map[string]bump

func (p packageFlags) String() string { return "name:patch|minor|major|none" }
func (p packageFlags) Set(value string) error {
	name, level, ok := strings.Cut(value, ":")
	if !ok || name == "" {
		return errors.New("use --package name:patch|minor|major|none")
	}
	if _, exists := p[name]; exists {
		return fmt.Errorf("duplicate package flag %q", name)
	}
	p[name] = bump(level)
	return nil
}
func writeRecord(root, id string, r record) (err error) {
	var contents strings.Builder
	contents.WriteString("---\n")
	for _, name := range sortedKeys(r.Packages) {
		quoted, _ := json.Marshal(name)
		fmt.Fprintf(&contents, "%s: %s\n", quoted, r.Packages[name])
	}
	contents.WriteString("---\n\n")
	contents.WriteString(strings.TrimSpace(r.Summary))
	contents.WriteString("\n")
	if strings.TrimSpace(r.Public) != "" {
		fmt.Fprintf(&contents, "\n%s\n%s\n%s\n", publicStart, strings.TrimSpace(r.Public), publicEnd)
	}
	data := []byte(contents.String())
	// Validate the rendered form before writing so reserved block syntax cannot
	// change the meaning of author-supplied summaries or public notes.
	var parsed record
	if err := decode(data, &parsed); err != nil {
		return err
	}
	// Exclusive creation protects existing records, including concurrent authors.
	path := filepath.Join(root, ".changeset", id+".md")
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0644)
	if err != nil {
		return err
	}
	defer func() {
		if err != nil {
			os.Remove(path)
		}
	}()
	_, err = f.Write(data)
	closeErr := f.Close()
	if err != nil {
		return err
	}
	return closeErr
}
func run(root string, args []string, out io.Writer) error {
	const usage = "usage: changeset packages | add --id ID --package NAME:BUMP --summary TEXT [--public TEXT] | check | status | changelog [--internal]"
	if len(args) == 0 || args[0] == "--help" || args[0] == "help" {
		_, err := fmt.Fprintln(out, usage)
		return err
	}
	command := args[0]
	if command != "packages" && command != "add" && command != "check" && command != "status" && command != "changelog" {
		return errors.New(usage)
	}
	flags := flag.NewFlagSet(command, flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	var id, summary, public string
	var internal bool
	selected := packageFlags{}
	if command == "add" {
		flags.StringVar(&id, "id", "", "record ID")
		flags.Var(selected, "package", "package name and bump")
		flags.StringVar(&summary, "summary", "", "internal Markdown")
		flags.StringVar(&public, "public", "", "public Markdown")
	}
	if command == "changelog" {
		flags.BoolVar(&internal, "internal", false, "show internal summaries")
	}
	if err := flags.Parse(args[1:]); err != nil {
		return err
	}
	if flags.NArg() != 0 {
		return errors.New("unexpected positional arguments")
	}
	packages, err := inventory(root)
	if err != nil {
		return err
	}
	if command == "packages" {
		return json.NewEncoder(out).Encode(packages)
	}
	if command == "add" {
		if !validID.MatchString(id) || id == "readme" {
			return errors.New("--id must be lowercase kebab-case and must not be readme")
		}
		if len(selected) == 0 || strings.TrimSpace(summary) == "" {
			return errors.New("add requires at least one --package and a nonblank --summary")
		}
		for _, text := range []string{summary, public} {
			if strings.Contains(text, publicStart) || strings.Contains(text, publicEnd) {
				return errors.New("summary and public text must not contain reserved public changelog markers")
			}
		}
		r := record{Packages: selected, Summary: summary, Public: public}
		if err := validate(r, packages); err != nil {
			return err
		}
		if err := writeRecord(root, id, r); err != nil {
			return err
		}
		_, err := fmt.Fprintf(out, "created .changeset/%s.md\n", id)
		return err
	}
	records, err := readPending(root, packages)
	if err != nil {
		return err
	}
	switch command {
	case "check":
		_, err = fmt.Fprintf(out, "validated %d changesets\n", len(records))
		return err
	case "status":
		return json.NewEncoder(out).Encode(records)
	case "changelog":
		notes := map[string][]string{}
		for _, r := range records {
			note := r.Public
			if internal {
				note = r.Summary
			}
			if strings.TrimSpace(note) == "" {
				continue
			}
			for name := range r.Packages {
				notes[name] = append(notes[name], strings.TrimSpace(note))
			}
		}
		var result strings.Builder
		if len(notes) > 0 {
			result.WriteString("# Unreleased\n")
		}
		for _, name := range sortedKeys(notes) {
			fmt.Fprintf(&result, "\n## %s\n\n%s\n", name, strings.Join(notes[name], "\n\n---\n\n"))
		}
		_, err = io.WriteString(out, result.String())
		return err
	}
	return nil
}
