# SPDX-License-Identifier: FSL-1.1-ALv2
FROM golang:1.26.8-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
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
