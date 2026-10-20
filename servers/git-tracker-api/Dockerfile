# Docker Hub has not published the Go 1.27.3 image yet. Bootstrap from 1.27.2
# and let go.mod select 1.27.3. Remove this override when that image is available.
FROM golang:1.27.2-bookworm AS build
ENV GOTOOLCHAIN=auto
WORKDIR /src
COPY go.mod go.sum ./
RUN go version && go mod download
COPY protos ./protos
COPY servers/git-tracker-api ./servers/git-tracker-api
RUN CGO_ENABLED=0 go build -trimpath -o /out/git-tracker-api ./servers/git-tracker-api/cmd/git-tracker-api
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/git-tracker-api /git-tracker-api
COPY servers/LICENSE servers/NOTICE /licenses/servers/
COPY LICENSE NOTICE /licenses/protocols/
USER 65532:65532
EXPOSE 46321
ENTRYPOINT ["/git-tracker-api"]
