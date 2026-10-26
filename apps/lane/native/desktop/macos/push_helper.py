#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Synthetic helper for push_registration_test.mm; never used in a product package.

LANE_PUSH_TEST_CONTROL names a scratch directory shared with the test:
  fail   - optional; "auth" answers RegisterPush with UNAUTHENTICATED, any other
           text with DEPENDENCY_UNAVAILABLE. The mode is read when the call starts.
  delay  - optional seconds to hold a RegisterPush call open after recording it.
  space  - the space ID the next `sync callback` reports as reconnected.
  calls  - appended JSON lines, one per RegisterPush call.
  workers - appended space IDs, one per `sync run` start.
"""
import json
import os
from pathlib import Path
import sys
import time

control = Path(os.environ["LANE_PUSH_TEST_CONTROL"])
args = sys.argv[1:]


def option(name):
    return args[args.index(name) + 1]


def read(name, default=""):
    path = control / name
    return path.read_text().strip() if path.exists() else default


result = {}
if "sync" in args and "run" in args:
    with open(control / "workers", "a") as workers:
        workers.write(option("--space") + "\n")
    while True:
        time.sleep(1)
if "rpc" in args and option("rpc") == "RegisterPush":
    body = json.load(sys.stdin)
    mode = read("fail")
    with open(control / "calls", "a") as calls:
        calls.write(json.dumps({"expectedSpace": option("--expected-space"), "request": body}) + "\n")
    time.sleep(float(read("delay", "0")))
    if mode:
        reason = "ERROR_REASON_UNAUTHENTICATED" if mode == "auth" else "ERROR_REASON_DEPENDENCY_UNAVAILABLE"
        print(json.dumps({"ok": False, "error": {"reason": reason, "message": "synthetic", "retryable": False}}))
        sys.exit(1)
elif "callback" in args:
    result = {"spaceId": read("space"), "sequence": "0"}
print(json.dumps({"ok": True, "result": result}))
