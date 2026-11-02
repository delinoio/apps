# SPDX-License-Identifier: FSL-1.1-ALv2
# Build from the repository root: the server is a package in the root Go module.
FROM golang:1.27.3-bookworm@sha256:d9b6a1beb9474adc2a1ce842573639b44b2a0dd49b6669f51d3821baf9ec37c9 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY protos ./protos
COPY servers/mcpa-api ./servers/mcpa-api
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/mcpa-api ./servers/mcpa-api
FROM gcr.io/distroless/static-debian12:nonroot@sha256:afa5c872c891853ca7fcf1f12c3edb23f7eeef36189728842dd51042ff57f7ab
LABEL org.opencontainers.image.title="mcpa-api" \
      org.opencontainers.image.licenses="FSL-1.1-ALv2"
COPY --from=build /out/mcpa-api /mcpa-api
COPY servers/LICENSE servers/NOTICE /licenses/servers/
COPY LICENSE NOTICE /licenses/protocols/
USER 65532:65532
EXPOSE 46329
ENTRYPOINT ["/mcpa-api"]
CMD ["serve"]
