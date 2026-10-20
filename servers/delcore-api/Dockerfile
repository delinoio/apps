# Docker Hub has not published the Go 1.27.3 image yet. Bootstrap from 1.27.2
# and let go.mod select 1.27.3. Remove this override when that image is available.
FROM golang:1.27.2-bookworm AS build
ENV GOTOOLCHAIN=auto
WORKDIR /src
COPY go.mod go.sum ./
RUN go version && go mod download
COPY protos ./protos
COPY servers/delcore-api ./servers/delcore-api
RUN CGO_ENABLED=0 go build -trimpath -o /out/delcore-api ./servers/delcore-api/cmd/delcore-api
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/delcore-api /delcore-api
COPY servers/LICENSE servers/NOTICE /licenses/servers/
COPY LICENSE NOTICE /licenses/protocols/
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/delcore-api"]
