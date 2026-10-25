#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Synthetic helper for reauth_test.mm; never used in a product package."""
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
challenge = "01900000-0000-7000-8000-000000000020"
result = {}
if "rpc" in args:
    method = args[args.index("rpc") + 1]
    body = json.load(sys.stdin)
    assert "reauthProof" not in body and "reauth_proof" not in body
    assert "challengeId" not in body
    if method == "BeginDataDeletion":
        result = {"challengeId": challenge, "authorizationUrl": "https://test.invalid/approve"}
    elif method == "DeleteAccountData":
        assert args[args.index("--reauth-challenge") + 1] == challenge
        marker = Path(os.environ["LANE_NATIVE_TEST_ROOT"]) / "helper-deleted"
        assert not marker.exists(), "No repeated final RPC"
        marker.touch()
        result = {"completed": True}
elif "callback" in args:
    result = {"challengeId": challenge}
elif "cancel-reauth" in args:
    assert args[args.index("--challenge-id") + 1] == challenge
    result = {"completed": True}
print(json.dumps({"ok": True, "result": result}))
