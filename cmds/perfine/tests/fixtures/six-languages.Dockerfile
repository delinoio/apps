# SPDX-License-Identifier: Apache-2.0
# These are fixture toolchains, not the CLI distribution or a production image.
ARG RUST_IMAGE=rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1
ARG NODE_IMAGE=node@sha256:55fb5f5a527ea2c5c4a95f963e48ef90aa38c7b346054fdc8ee9957a2b55d205
ARG GO_IMAGE=golang@sha256:972b4110ba0f47539a975c31e52c64765e1ff19f2d74469ae4a595c6676e80a5
FROM ${NODE_IMAGE} AS node
FROM ${GO_IMAGE} AS go
FROM ${RUST_IMAGE}
RUN apt-get update && apt-get install -y --no-install-recommends valgrind util-linux ca-certificates curl xz-utils && rm -rf /var/lib/apt/lists/*
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=go /usr/local/go /usr/local/go
RUN curl --fail --location --retry 2 --output /tmp/zig.tar.xz https://ziglang.org/download/0.15.2/zig-x86_64-linux-0.15.2.tar.xz && \
    echo '02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239  /tmp/zig.tar.xz' | sha256sum --check && \
    tar -xJf /tmp/zig.tar.xz -C /opt && rm /tmp/zig.tar.xz && \
    ln -s /opt/zig-x86_64-linux-0.15.2/zig /usr/local/bin/zig
ENV PATH="/usr/local/go/bin:/usr/local/cargo/bin:${PATH}"
# DockerEnvironment clears inherited environment. Expose toolchain executables
# through its fixed safe PATH; caches remain in the task-owned /build directory.
RUN ln -s /usr/local/go/bin/go /usr/local/bin/go && \
    ln -s /usr/local/rustup/toolchains/*/bin/rustc /usr/local/bin/rustc
