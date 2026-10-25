# SPDX-License-Identifier: Apache-2.0
"""Run the built macOS host with a synthetic bridge bundle and an independent Go writer.

Requires a LANE_NATIVE_TESTING=ON host, the native-smoke.config.ts bundle,
and a compiled CLI. All evidence and data stay in the supplied scratch directory.
This tests the native bridge and persistence, not the complete product UI.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument("--app", required=True, type=Path)
parser.add_argument("--bundle", required=True, type=Path)
parser.add_argument("--cli", required=True, type=Path)
parser.add_argument("--scratch", required=True, type=Path)
args = parser.parse_args()
args.scratch.mkdir(parents=True, exist_ok=True)
app = args.scratch / "Lane.app"
if app.exists():
    raise SystemExit("Use a fresh scratch directory")
shutil.copytree(args.app, app)
shutil.copyfile(args.bundle, app / "Contents/Resources/main.lynx.bundle")
home = args.scratch / "home"
root = home / "Library/Application Support/io.delino.lane"
env = dict(os.environ, HOME=str(home), LANE_NATIVE_TEST_ROOT=str(root), NSUnbufferedIO="YES")
task_id = "01900000-0000-7000-8000-000000000001"


def cli(*arguments, request=None):
    command = [str(args.cli.resolve()), "--json", *arguments]
    if request is not None:
        command += ["--request-json", "-"]
    result = subprocess.run(command, input=json.dumps(request) if request else None,
                            env=env, text=True, capture_output=True, timeout=20)
    value = json.loads(result.stdout)
    assert result.returncode == 0 and value.get("ok"), value
    return value["result"]


def await_log(path, marker, process, after=0):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        text = path.read_text(errors="replace")
        if "LANE_NATIVE_TEST_FAILED" in text:
            raise AssertionError(text)
        if marker in text[after:]:
            return text
        assert process.poll() is None, f"Host exited: {process.returncode}"
        time.sleep(0.1)
    raise AssertionError(f"Missing {marker}: {path}")


results = []
for iteration in range(2):
    log = args.scratch / f"run-{iteration}.log"
    with log.open("w") as output:
        host = subprocess.Popen([str(app / "Contents/MacOS/Lane")], env=env,
                                stdout=output, stderr=output)
        try:
            text = await_log(log, "LANE_NATIVE_READY", host)
            expected = "RESULT_KIND_APPLIED" if iteration == 0 else "RESULT_KIND_REPLAYED"
            assert expected in text, text
            version = cli("task", "get", "--id", task_id)["result"]["version"]
            assert version["task"]["prompt"] == "Synthetic Markdown **prompt**"
            version["task"]["title"] = "external commit"
            offset = len(log.read_text())
            cli("task", "edit", request={
                "meta": {"requestId": f"01900000-0000-7000-8000-{iteration+100:012d}"},
                "id": task_id, "expectedVersionId": version["versionId"], "task": version["task"],
            })
            await_log(log, "LANE_NATIVE_EVENT", host, offset)
            await_log(log, "LANE_NATIVE_EXTERNAL_COMMIT_OBSERVED", host, offset)
            results.append({"launch": iteration, "creation": expected,
                            "goReadNativeWrite": True, "nativeObservedGoWrite": True})
        finally:
            host.terminate()
            host.wait(timeout=10)
(args.scratch / "result.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps(results))
