// SPDX-License-Identifier: Apache-2.0
package main

import (
	"context"
	"github.com/delinoio/apps/cmds/lane/internal/cli"
	laneruntime "github.com/delinoio/apps/cmds/lane/internal/runtime"
	"os"
	"os/signal"
	"syscall"
)

var version = "0.1.0"

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	r := &laneruntime.Runtime{Diagnostics: os.Stderr, Version: version}
	defer r.Close()
	a := cli.App{Version: version, In: os.Stdin, Out: os.Stdout, Err: os.Stderr, Invoke: r.Invoke, MCP: r.ServeMCP}
	code := a.Run(ctx, os.Args[1:])
	r.Close()
	os.Exit(code)
}
