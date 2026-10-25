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
shutil.copytree(args.app, app, symlinks=True)
shutil.copyfile(args.bundle, app / "Contents/Resources/app/main.lynx.bundle")
home = args.scratch / "home"
root = home / "Library/Application Support/io.delino.lane"
env = dict(os.environ, HOME=str(home), LANE_NATIVE_TEST_ROOT=str(root), NSUnbufferedIO="YES",
           LANE_NATIVE_TEST_LIFECYCLE="1", LANE_NATIVE_TEST_AUTOQUIT="1")
host_path = app / "Contents/MacOS/lynxtron"
helper_path = app / "Contents/Resources/app/helpers/lane"
assert host_path.resolve() != helper_path.resolve()
assert not os.path.samefile(host_path, helper_path), "Host and helper must be separate files"
helper_version = subprocess.check_output([str(helper_path), "--version"], env=env,
                                         text=True, timeout=10).strip()
assert helper_version == "lane 0.1.0", helper_version
mcp = subprocess.run([str(helper_path), "mcp", "serve"], input=json.dumps({
    "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "native-package-validation", "version": "1"},
    },
}) + "\n", env=env, text=True, capture_output=True, timeout=20)
assert mcp.returncode == 0, mcp.stderr
messages = [json.loads(line) for line in mcp.stdout.splitlines() if line.strip()]
assert any(item.get("id") == 1 and "result" in item for item in messages), messages
(args.scratch / "packaging.json").write_text(json.dumps({
    "host": "Contents/MacOS/lynxtron", "helper": "Contents/Resources/app/helpers/lane",
    "distinctFiles": True, "helperVersion": helper_version, "helperMcpInitialize": True,
}, indent=2) + "\n")
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
        host = subprocess.Popen([str(app / "Contents/MacOS/lynxtron")], env=env,
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
            await_log(log, "LANE_NATIVE_WINDOW_CLOSED", host)
            closed_offset = len(log.read_text())
            # The independent CLI remains usable while the visible host is gone.
            closed_read = cli("task", "get", "--id", task_id)
            assert closed_read["result"]["version"]["task"]["title"] == "external commit"
            await_log(log, "LANE_NATIVE_WINDOW_REOPENED", host, closed_offset)
            await_log(log, "LANE_NATIVE_READY", host, closed_offset)
            await_log(log, '"event":"lane_stopped"', host)
            assert host.wait(timeout=10) == 0, "Explicit Quit must exit successfully"
            results.append({"launch": iteration, "windowCloseReopen": True, "creation": expected,
                            "goReadNativeWrite": True, "nativeObservedGoWrite": True})
        finally:
            if host.poll() is None:
                host.terminate()
                try:
                    host.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    host.kill()
                    host.wait(timeout=5)
(args.scratch / "result.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps(results))
