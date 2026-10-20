# SPDX-License-Identifier: FSL-1.1-ALv2
# Docker Hub has not published the repository Go 1.27.3 image yet.
# Bootstrap with 1.27.2 and let go.mod select the exact toolchain.
# Remove this override when the matching image is available.
FROM golang:1.27.2-bookworm AS build
ENV GOTOOLCHAIN=auto
WORKDIR /src
COPY go.mod go.sum ./
RUN go version && go mod download
COPY protos ./protos
COPY servers/delitools-api ./servers/delitools-api
RUN CGO_ENABLED=0 go build -trimpath -o /out/delitools-api ./servers/delitools-api/cmd/delitools-api
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/delitools-api /delitools-api
COPY servers/LICENSE servers/NOTICE /licenses/servers/
COPY LICENSE NOTICE /licenses/protocols/
USER 65532:65532
EXPOSE 46322
ENTRYPOINT ["/delitools-api"]
