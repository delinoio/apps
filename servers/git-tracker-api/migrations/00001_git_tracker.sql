-- SPDX-License-Identifier: FSL-1.1-ALv2

CREATE TABLE git_tracker_repositories (
 id uuid PRIMARY KEY,
 github_id bigint NOT NULL,
 space_id uuid NOT NULL,
 full_name text NOT NULL,
 installation_id text NOT NULL,
 public boolean NOT NULL,
 connected boolean NOT NULL DEFAULT true,
 state jsonb NOT NULL DEFAULT '{"next":0,"records":{}}'
);
CREATE TABLE git_tracker_tokens (
 id uuid PRIMARY KEY,
 repository_id uuid NOT NULL REFERENCES git_tracker_repositories(id) ON DELETE CASCADE,
 digest bytea NOT NULL UNIQUE,
 expires_at timestamptz NOT NULL,
 revoked boolean NOT NULL DEFAULT false
);
CREATE TABLE git_tracker_audit (
 id uuid PRIMARY KEY,
 space_id uuid NOT NULL,
 action text NOT NULL,
 correlation_id uuid NOT NULL,
 created_at timestamptz NOT NULL
);

CREATE TABLE git_tracker_links (
 space_id uuid NOT NULL,
 user_id uuid NOT NULL,
 github_id text NOT NULL,
 PRIMARY KEY(space_id,user_id)
);

CREATE UNIQUE INDEX git_tracker_active_repository ON git_tracker_repositories(github_id) WHERE connected;
CREATE UNIQUE INDEX git_tracker_space_repository ON git_tracker_repositories(github_id,space_id);
