# SPDX-License-Identifier: FSL-1.1-ALv2
FROM rust:1.95-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY protos ./protos
COPY servers ./servers
RUN cargo build --locked --release --manifest-path servers/Cargo.toml -p delcore-api --bin delcore-api
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/servers/target/release/delcore-api /delcore-api
COPY servers/LICENSE servers/NOTICE /licenses/servers/
COPY LICENSE NOTICE /licenses/protocols/
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/delcore-api"]
