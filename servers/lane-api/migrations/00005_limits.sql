-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE IF NOT EXISTS lane_rate_limits (
    scope text NOT NULL,
    identity text NOT NULL,
    window_start timestamptz NOT NULL,
    requests bigint NOT NULL CHECK (requests > 0),
    PRIMARY KEY (scope, identity)
);
