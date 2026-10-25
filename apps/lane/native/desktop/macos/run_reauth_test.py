# SPDX-License-Identifier: Apache-2.0
"""Run the real macOS runtime with an isolated synthetic helper and store."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix="lane-native-reauth-") as scratch:
    root = Path(scratch)
    binary = root / "LaneReauthTest"
    shutil.copy2(sys.argv[1], binary)
    (root / "helpers").mkdir()
    helper = root / "helpers/lane"
    shutil.copy2(Path(__file__).with_name("reauth_helper.py"), helper)
    helper.chmod(0o700)
    result = subprocess.run([str(binary)], env=dict(os.environ,
        LANE_NATIVE_TEST_ROOT=str(root / "data")), timeout=45)
    sys.exit(result.returncode)
