// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"github.com/jackc/pgx/v5"
	"io"
	"net/http"
	"strconv"
	"strings"
)

func (s *Server) Maintain(ctx context.Context) error {
	if lifecycle, ok := s.Auth.(interface {
		Finalized(context.Context, []string) ([]string, error)
	}); ok {
		rows, e := s.DB.Query(ctx, "SELECT DISTINCT space_id::text FROM git_tracker_repositories ORDER BY space_id::text")
		if e != nil {
			return e
		}
		spaces := []string{}
		for rows.Next() {
			var space string
			if e = rows.Scan(&space); e != nil {
				rows.Close()
				return e
			}
			spaces = append(spaces, space)
		}
		e = rows.Err()
		rows.Close()
		if e != nil {
			return e
		}
		for start := 0; start < len(spaces); start += 100 {
			end := start + 100
			if end > len(spaces) {
				end = len(spaces)
			}
			deleted, e := lifecycle.Finalized(ctx, spaces[start:end])
			if e != nil {
				s.Log.WarnContext(ctx, "tenant lifecycle unavailable", "category", "authorization")
				continue
			}
			if len(deleted) > 0 {
				tx, e := s.DB.Begin(ctx)
				if e != nil {
					return e
				}
				_, e = tx.Exec(ctx, "DELETE FROM git_tracker_repositories WHERE space_id=ANY($1::uuid[])", deleted)
				if e == nil {
					_, e = tx.Exec(ctx, "DELETE FROM git_tracker_audit WHERE space_id=ANY($1::uuid[])", deleted)
				}
				if e == nil {
					_, e = tx.Exec(ctx, "DELETE FROM git_tracker_links WHERE space_id=ANY($1::uuid[])", deleted)
				}
				if e == nil {
					e = tx.Commit(ctx)
				} else {
					tx.Rollback(ctx)
				}
				if e != nil {
					return e
				}
			}
		}
	}
	rows, e := s.DB.Query(ctx, "SELECT id::text FROM git_tracker_repositories ORDER BY id")
	if e != nil {
		return e
	}
	ids := []string{}
	for rows.Next() {
		var id string
		if e = rows.Scan(&id); e != nil {
			rows.Close()
			return e
		}
		ids = append(ids, id)
	}
	e = rows.Err()
	rows.Close()
	if e != nil {
		return e
	}
	for _, rid := range ids {
		tx, e := s.DB.Begin(ctx)
		if e != nil {
			return e
		}
		repo, state, e := s.repository(ctx, tx, rid)
		if e == nil && repo.Connected {
			if github, ok := s.GitHub.(*GitHubClient); ok {
				token, lookup := github.installation(ctx, repo.InstallationId)
				var current githubRepository
				if lookup == nil {
					lookup = github.call(ctx, "GET", repositoryIDPath(repo), token, nil, &current)
				}
				if lookup == nil && current.ID != 0 {
					if repo.Public == current.Private || repo.FullName != current.FullName {
						_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET public=$2,full_name=$3 WHERE id=$1", rid, !current.Private, current.FullName)
						if e == nil {
							e = s.audit(ctx, tx, repo.SpaceId, "repository_metadata_changed")
						}
					}
				} else {
					s.event("github", "failure")
				}
			}
		}
		count := 0
		if e == nil {
			count = state.Interrupt(s.now())
			if count > 0 {
				var raw []byte
				raw, e = json.Marshal(state)
				if e == nil {
					_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET state=$2 WHERE id=$1", rid, raw)
				}
			}
		}
		if e == nil {
			e = tx.Commit(ctx)
		} else {
			tx.Rollback(ctx)
		}
		if e != nil {
			return e
		}
		if count > 0 {
			for i := 0; i < count; i++ {
				s.event("execution", "interrupted")
			}
			s.Log.InfoContext(ctx, "executions interrupted", "count", count)
		}
	}
	_, e = s.DB.Exec(ctx, "DELETE FROM git_tracker_audit WHERE created_at<$1", s.now().Add(-s.AuditRetention))
	return e
}
func (s *Server) Webhook(secret string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		if r.Method != "POST" {
			w.WriteHeader(http.StatusMethodNotAllowed)
			return
		}
		raw, e := io.ReadAll(http.MaxBytesReader(w, r.Body, 2<<20))
		if e != nil {
			w.WriteHeader(http.StatusRequestEntityTooLarge)
			return
		}
		signature, e := hex.DecodeString(strings.TrimPrefix(r.Header.Get("X-Hub-Signature-256"), "sha256="))
		mac := hmac.New(sha256.New, []byte(secret))
		mac.Write(raw)
		if e != nil || !hmac.Equal(signature, mac.Sum(nil)) {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		var event struct {
			Action       string           `json:"action"`
			Repository   githubRepository `json:"repository"`
			Installation struct {
				ID int64 `json:"id"`
			} `json:"installation"`
			Removed []githubRepository `json:"repositories_removed"`
		}
		if json.Unmarshal(raw, &event) != nil {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		ctx := r.Context()
		tx, e := s.DB.Begin(ctx)
		if e != nil {
			w.WriteHeader(503)
			return
		}
		defer tx.Rollback(ctx)
		switch r.Header.Get("X-GitHub-Event") {
		case "installation":
			if event.Action == "deleted" || event.Action == "suspend" {
				_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false,public=false WHERE installation_id=$1", strconv.FormatInt(event.Installation.ID, 10))
			}
		case "installation_repositories":
			for _, removed := range event.Removed {
				if _, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false,public=false WHERE github_id=$1 AND installation_id=$2", strconv.FormatInt(removed.ID, 10), strconv.FormatInt(event.Installation.ID, 10)); e != nil {
					break
				}
			}
		case "repository":
			if event.Action == "deleted" {
				_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false,public=false WHERE github_id=$1", strconv.FormatInt(event.Repository.ID, 10))
			} else {
				// Re-fetch current metadata instead of trusting event delivery order.
				var repoID, install string
				e = tx.QueryRow(ctx, "SELECT id::text,installation_id FROM git_tracker_repositories WHERE github_id=$1 AND connected FOR UPDATE", strconv.FormatInt(event.Repository.ID, 10)).Scan(&repoID, &install)
				if errors.Is(e, pgx.ErrNoRows) {
					e = nil
					break
				}
				if e == nil {
					if g, ok := s.GitHub.(*GitHubClient); ok {
						token, err := g.installation(ctx, install)
						var current githubRepository
						if err == nil {
							err = g.call(ctx, "GET", "/repositories/"+strconv.FormatInt(event.Repository.ID, 10), token, nil, &current)
						}
						if err == nil {
							_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET public=$2,full_name=$3 WHERE id=$1", repoID, !current.Private, current.FullName)
						} else {
							e = err
						}
					}
				}
			}
		}
		if e == nil {
			eventType := r.Header.Get("X-GitHub-Event")
			if eventType == "repository" || eventType == "installation" || eventType == "installation_repositories" {
				rows, err := tx.Query(ctx, "SELECT DISTINCT space_id::text FROM git_tracker_repositories WHERE ($1<>0 AND github_id=$1) OR ($2<>'0' AND installation_id=$2)", event.Repository.ID, strconv.FormatInt(event.Installation.ID, 10))
				e = err
				if e == nil {
					spaces := []string{}
					for rows.Next() {
						var space string
						if e = rows.Scan(&space); e != nil {
							break
						}
						spaces = append(spaces, space)
					}
					if e == nil {
						e = rows.Err()
					}
					rows.Close()
					if e == nil {
						for _, space := range spaces {
							if e = s.audit(ctx, tx, space, "github_connection_event"); e != nil {
								break
							}
						}
					}
				}
			}
		}
		if e == nil {
			e = tx.Commit(ctx)
		}
		if e != nil {
			s.event("github", "failure")
			w.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	})
}
