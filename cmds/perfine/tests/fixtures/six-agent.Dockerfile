# SPDX-License-Identifier: Apache-2.0
ARG BASE
FROM ${BASE}
COPY agent.mjs /fixture/agent.mjs
COPY candidate.* /fixture/
COPY baseline.* /fixture/
RUN printf '#!/bin/sh\nexec node /fixture/agent.mjs "$@"\n' > /usr/local/bin/mock-agent && chmod 755 /usr/local/bin/mock-agent
