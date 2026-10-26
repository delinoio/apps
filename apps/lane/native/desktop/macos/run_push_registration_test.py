# SPDX-License-Identifier: Apache-2.0
"""Run each APNs registration scenario in a fresh process with a synthetic helper."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

SCENARIOS = [
    "retained-account", "transient-failure", "authentication-failure", "selection-mismatch",
    "reconnect-during-attempt", "healthy-counterfactual",
    "token-change", "in-flight-supersession", "disconnect-before-retry", "disable-before-retry",
    "remove-before-retry", "stop-before-retry", "account-isolation",
]
failed = []
for scenario in SCENARIOS:
    with tempfile.TemporaryDirectory(prefix="lane-native-push-") as scratch:
        root = Path(scratch)
        (root / "control").mkdir()
        (root / "helpers").mkdir()
        helper = root / "helpers/lane"
        helper.write_bytes(Path(__file__).with_name("push_helper.py").read_bytes())
        helper.chmod(0o700)
        result = subprocess.run([sys.argv[1], scenario], timeout=60, env=dict(os.environ,
            LANE_NATIVE_TEST_ROOT=str(root / "data"), LANE_PUSH_TEST_CONTROL=str(root / "control"),
            LANE_PUSH_TEST_HELPER=str(helper)))
        if result.returncode != 0:
            failed.append(scenario)
if failed:
    print("failed scenarios: " + ", ".join(failed), file=sys.stderr)
    sys.exit(1)
