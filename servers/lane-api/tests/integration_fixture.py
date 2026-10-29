#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-ALv2
"""Run Lane's ignored integration tests with a disposable private S3 mock."""
import importlib.metadata
import json
import logging
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import parse_qs, urlsplit

MOTO_VERSION = "5.1.22"
ROOT = Path(__file__).resolve().parents[3]
HELPER = Path(__file__).resolve()
CHILDREN = set()


def fixture_environment(database):
    parsed = urlsplit(database)
    if (parsed.scheme not in ("postgres", "postgresql")
            or parsed.hostname not in ("127.0.0.1", "localhost", "::1")
            or not parsed.path.strip("/") or parsed.fragment
            or set(parse_qs(parsed.query, keep_blank_values=True)) - {"sslmode"}):
        raise ValueError("TEST_DATABASE_URL must select a disposable loopback PostgreSQL database")
    # Ignore any inherited cloud endpoints or credentials. The mock and tests get
    # only synthetic S3 credentials, even on a developer machine with AWS settings.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("AWS_", "LANE_R2_", "MOTO_"))}
    env.update(
        TEST_DATABASE_URL=database,
        LANE_TEST_DATABASE_URL=database,
        LANE_PUSH_TEST_DATABASE_URL=database,
        LANE_R2_BUCKET="lane-integration-fixture",
        LANE_R2_ACCESS_KEY_ID="fixture-access",
        LANE_R2_SECRET_ACCESS_KEY="fixture-secret-password",
        AWS_ACCESS_KEY_ID="fixture-access",
        AWS_SECRET_ACCESS_KEY="fixture-secret-password",
        AWS_EC2_METADATA_DISABLED="true",
        NO_PROXY="127.0.0.1,localhost,::1",
        no_proxy="127.0.0.1,localhost,::1",
    )
    return env


def stop(process):
    # Kill the group, including Cargo test executables and mock worker children.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    except PermissionError:
        # macOS can report EPERM instead of ESRCH for a group whose leader was
        # already reaped. Never signal a reused, inaccessible process group.
        if process.poll() is None:
            raise
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        process.poll()
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            break
        except PermissionError:
            if process.poll() is None:
                raise
            break
        time.sleep(0.05)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            if process.poll() is None:
                raise
    process.wait()
    CHILDREN.discard(process)


def launch(command, env, **kwargs):
    process = subprocess.Popen(command, cwd=ROOT, env=env,
                               start_new_session=True, **kwargs)
    CHILDREN.add(process)
    return process


def run(command, env):
    process = launch(command, env)
    try:
        return process.wait()
    finally:
        stop(process)


def serve():
    if importlib.metadata.version("moto") != MOTO_VERSION:
        raise RuntimeError("unexpected Moto fixture version")
    import boto3
    from botocore.config import Config
    from moto.server import ThreadedMotoServer

    # Request logs contain signed capability URLs, so suppress them even in fixtures.
    logging.getLogger("werkzeug").disabled = True
    server = ThreadedMotoServer(ip_address="127.0.0.1", port=0, verbose=False)
    server.start()
    host, port = server.get_host_and_port()
    endpoint = f"http://{host}:{port}"
    client = boto3.client(
        "s3", endpoint_url=endpoint, region_name="auto",
        aws_access_key_id="fixture-access",
        aws_secret_access_key="fixture-secret-password",
        config=Config(signature_version="s3v4", retries={"max_attempts": 0}),
    )
    client.create_bucket(
        Bucket=os.environ["LANE_R2_BUCKET"],
        CreateBucketConfiguration={"LocationConstraint": "auto"},
    )
    client.head_bucket(Bucket=os.environ["LANE_R2_BUCKET"])
    print(json.dumps({"endpoint": endpoint, "version": MOTO_VERSION}), flush=True)
    threading.Event().wait()


def interrupted(signum, _frame):
    raise InterruptedError(f"fixture interrupted by signal {signum}")


def main():
    if sys.argv[1:] == ["--serve"]:
        serve()
        return 0
    if sys.argv[1:]:
        raise ValueError("run without arguments; configure TEST_DATABASE_URL")
    env = fixture_environment(os.environ.get("TEST_DATABASE_URL", ""))
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        with tempfile.TemporaryDirectory(prefix="lane-integration-") as directory:
            python = Path(directory) / "venv" / "bin" / "python"
            if run([sys.executable, "-m", "venv", str(python.parent.parent)], env):
                raise RuntimeError("cannot create isolated fixture environment")
            if run([str(python), "-m", "pip", "--isolated", "--disable-pip-version-check",
                    "install", "--quiet", f"moto[server]=={MOTO_VERSION}"], env):
                raise RuntimeError("cannot install pinned Moto fixture")
            mock = launch([str(python), str(HELPER), "--serve"], env,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
            try:
                if not select.select([mock.stdout], [], [], 30)[0]:
                    raise RuntimeError("S3 fixture startup timed out")
                ready = json.loads(mock.stdout.readline())
                parsed = urlsplit(ready["endpoint"])
                if (ready["version"] != MOTO_VERSION or parsed.scheme != "http"
                        or parsed.hostname != "127.0.0.1" or not parsed.port):
                    raise RuntimeError("invalid S3 fixture readiness")
                env["LANE_R2_ENDPOINT"] = ready["endpoint"]
                print(json.dumps({"event": "lane_s3_fixture_ready", "version": MOTO_VERSION,
                                  "host": "127.0.0.1", "port": parsed.port}), flush=True)
                return run(["cargo", "test", "--locked", "--manifest-path", "servers/Cargo.toml",
                            "-p", "lane-api", "--", "--ignored", "--nocapture"], env)
            finally:
                stop(mock)
                mock.stdout.close()
    finally:
        for process in list(CHILDREN):
            stop(process)
        print(json.dumps({"event": "lane_s3_fixture_stopped"}), flush=True)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, RuntimeError, InterruptedError, OSError):
        # Do not echo database URLs, inherited settings or signed request details.
        print("Lane integration fixture failed; inspect preceding test results.", file=sys.stderr)
        sys.exit(1)
