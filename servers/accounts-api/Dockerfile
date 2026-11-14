# SPDX-License-Identifier: FSL-1.1-ALv2
# accounts-api production image.
# Contract: docs/servers-accounts-api-foundation.md
# Build context: the repository root, which holds the root Go module.
#   docker build -f servers/accounts-api/Dockerfile .

FROM golang:1.26.1-alpine AS build
WORKDIR /src
# go.sum is optional until the module has external dependencies.
COPY go.mod go.sum* ./
RUN go mod download
COPY servers/accounts-api/ servers/accounts-api/
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/accounts-api ./servers/accounts-api/cmd/accounts-api

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /out/accounts-api /accounts-api
# Listen on every interface inside the container. The Railway service sets
# PORT to the same port so health checks and public routing reach it.
ENV ADDR=:8080
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/accounts-api"]
