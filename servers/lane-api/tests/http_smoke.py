# SPDX-License-Identifier: FSL-1.1-ALv2
"""Run the real binary and Connect/MCP adapters against disposable PostgreSQL and Delcore mocks.

Requires LANE_TEST_DATABASE_URL, psql, and an already-built LANE_TEST_BINARY.
The database must be disposable and dedicated to this test.
"""
import hashlib
import json
import os
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

DB = os.environ["LANE_TEST_DATABASE_URL"]
BINARY = os.environ["LANE_TEST_BINARY"]
ACCOUNT = "01900000-0000-7000-8000-000000000001"
TENANT = "01900000-0000-7000-8000-000000000002"
DEVICE = "01900000-0000-7000-8000-000000000003"
SESSION = "01900000-0000-7000-8000-000000000004"
EPOCH = "01900000-0000-7000-8000-000000000005"
TOKEN = "a" * 43
MCP_TOKEN = "b" * 43
METRICS_TOKEN = "synthetic-metrics-http-fixture-token"
seen = []
state = "TENANT_STATE_ACTIVE"


class Mock(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.reply({"issuer": mock_url, "authorization_endpoint": mock_url + "/authorize", "token_endpoint": mock_url + "/token", "jwks_uri": mock_url + "/jwks"})

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length", "0")))
        if self.path == "/token":
            return self.reply({"access_token": "synthetic-m2m", "expires_in": 3600})
        body = json.loads(raw)
        seen.append((self.path, body, self.headers.get("authorization")))
        if self.path.endswith("/Authorize"):
            assert body == {"tenantId": TENANT, "userId": ACCOUNT, "capability": "manage", "paid": False}
            return self.reply({"allowed": True})
        if self.path.endswith("/GetTenantStates"):
            return self.reply({"states": [{"tenantId": TENANT, "state": state}]})
        self.send_error(500)

    def reply(self, value):
        encoded = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


def sql(statement):
    subprocess.run(["psql", DB, "-v", "ON_ERROR_STOP=1", "-q", "-c", statement], check=True, stdout=subprocess.DEVNULL)


def call(path, body=None, token=TOKEN, headers=None):
    h = {"authorization": "Bearer " + token, "content-type": "application/json", "connect-protocol-version": "1"}
    h.update(headers or {})
    request = urllib.request.Request(api + path, data=None if body is None else json.dumps(body).encode(), headers=h)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            data = response.read()
            return response.status, json.loads(data) if data and "json" in response.headers.get("content-type", "") else data.decode()
    except urllib.error.HTTPError as error:
        data = error.read()
        return error.code, json.loads(data) if data and "json" in error.headers.get("content-type", "") else data.decode()


counter = 10


def identity():
    global counter
    counter += 1
    return f"01900000-0000-7000-8000-{counter:012x}"


mock = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
mock_url = f"http://127.0.0.1:{mock.server_port}"
threading.Thread(target=mock.serve_forever, daemon=True).start()
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
api = f"http://127.0.0.1:{port}"
env = dict(os.environ, DATABASE_URL=DB, LISTEN=f"127.0.0.1:{port}", LANE_PUBLIC_URL=api, WEB_ORIGIN=api,
           LANE_METRICS_TOKEN=METRICS_TOKEN, MAX_ITEM_BYTES="4096", MAX_WEBHOOK_BYTES="4096", REQUESTS_PER_MINUTE="1000", WEBHOOK_REQUESTS_PER_MINUTE="1000", AUTH_REQUESTS_PER_MINUTE="1000",
           LOGTO_ISSUER=mock_url, LANE_LOGTO_CLIENT_ID="fixture", LANE_LOGTO_CLIENT_SECRET="fixture", DELCORE_RESOURCE="fixture", DELCORE_URL=mock_url, LOGTO_TOKEN_URL=mock_url + "/token", CLIENT_ID="fixture", CLIENT_SECRET="fixture",
           LANE_WEB_RETURN_URLS=json.dumps([api + "/callback"]), LANE_OAUTH_CLIENTS=json.dumps({"fixture": {"name": "fixture", "redirect_uris": [api + "/callback"], "origin_kind": "ORIGIN_KIND_CLI", "resource": api}}),
           LANE_WEBHOOK_ENCRYPTION_KEY="CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg", LANE_WEBHOOK_BASE_URL=api,
           LANE_APNS_ENABLED="false", LANE_FCM_ENABLED="false", LANE_WEB_PUSH_ENABLED="false")
subprocess.run([BINARY, "migrate"], env=env, check=True, stdout=subprocess.DEVNULL)
subprocess.run([BINARY, "check-schema"], env=env, check=True, stdout=subprocess.DEVNULL)
sql(f"INSERT INTO lane_accounts VALUES('{ACCOUNT}','{TENANT}','{EPOCH}'); INSERT INTO lane_devices(account_id,id,name) VALUES('{ACCOUNT}','{DEVICE}','fixture');")
for session, token, resource, kind in [(SESSION, TOKEN, api, "CLI"), (identity(), MCP_TOKEN, api + "/mcp", "MCP")]:
    sql(f"INSERT INTO lane_sessions(id,account_id,device_id,client_id,origin_kind,resource,issuer,csrf_hash,redirect_uri,epoch) VALUES('{session}','{ACCOUNT}','{DEVICE}','fixture','ORIGIN_KIND_{kind}','{resource}','{api}','fixture','{api}/callback','{EPOCH}'); INSERT INTO lane_credentials VALUES('{hashlib.sha256(token.encode()).hexdigest()}','{session}','access',clock_timestamp()+interval '1 hour',NULL);")
log = Path(os.environ.get("LANE_HTTP_TEST_LOG", "/tmp/lane-http-fixture.log"))
with log.open("w") as output:
    process = subprocess.Popen([BINARY, "serve"], env=env, stdout=output, stderr=output)
    try:
        for _ in range(100):
            if process.poll() is not None:
                raise AssertionError("server exited during startup; inspect fixture log")
            try:
                if call("/health/ready")[0] == 200:
                    break
            except OSError:
                time.sleep(0.05)
        else:
            raise AssertionError("readiness timeout")
        assert call("/metrics")[0] == 401
        assert call("/metrics", token=METRICS_TOKEN)[0] == 200
        info = call("/lane.v1.SyncService/GetSyncInfo", {})
        assert info[0] == 200 and info[1]["epoch"] == EPOCH, info
        repository = identity()
        request = {"meta": {"requestId": identity()}, "id": repository, "url": "https://example.com/a/b"}
        first = call("/lane.v1.LaneService/CreateRepository", request)
        assert first[0] == 200, first
        assert call("/lane.v1.LaneService/CreateRepository", request)[1]["result"]["version"]["versionId"] == first[1]["result"]["version"]["versionId"]
        task = identity()
        created = call("/lane.v1.LaneService/CreateTask", {"meta": {"requestId": identity()}, "id": task, "repository": {"id": repository}, "title": "fixture"})
        assert created[0] == 200, created
        version = created[1]["result"]["version"]["versionId"]
        progress = {"meta": {"requestId": identity()}, "id": task, "expectedVersionId": version, "progress": "PROGRESS_DONE"}
        assert call("/lane.v1.LaneService/SetProgress", progress)[0] == 200
        progress["meta"]["requestId"] = identity()
        progress["progress"] = "PROGRESS_BLOCKED"
        conflict = call("/lane.v1.LaneService/SetProgress", progress)
        assert conflict[0] == 409 and conflict[1]["code"] == "aborted", conflict
        retained = call("/lane.v1.LaneService/GetTask", {"id": task})
        assert retained[1]["result"]["result"] == "RESULT_KIND_CONFLICT"
        begin = {"meta": {"requestId": identity()}, "mode": "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL", "confirmedAccountId": ACCOUNT}
        challenge = call("/lane.v1.AccountService/BeginDataDeletion", begin)
        assert challenge[0] == 200, challenge
        assert call("/lane.v1.AccountService/BeginDataDeletion", begin)[1] == challenge[1]
        begin["mode"] = "DATA_DELETION_MODE_DELETE_EVERYWHERE"
        assert call("/lane.v1.AccountService/BeginDataDeletion", begin)[0] == 409
        init = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fixture", "version": "1"}}}
        accept = {"accept": "application/json, text/event-stream"}
        assert call("/mcp", init, headers=accept)[0] == 401
        assert call("/mcp", init, token=MCP_TOKEN, headers=accept)[0] == 200
        assert call("/lane.v1.LaneService/ListRepositories", {}, token=MCP_TOKEN)[0] == 401
        state = "TENANT_STATE_BLOCKED"
        assert call("/lane.v1.LaneService/ListRepositories", {})[0] == 403
        state = "TENANT_STATE_ACTIVE"
        sql(f"UPDATE lane_sessions SET revoked_at=clock_timestamp() WHERE id='{SESSION}'")
        assert call("/lane.v1.LaneService/ListRepositories", {})[0] == 401
        assert seen and all(auth == "Bearer synthetic-m2m" for _, _, auth in seen)
        metrics = call("/metrics", token=METRICS_TOKEN)[1]
        assert 'lane_push_attempts_total{result="transient_failure"} 0' in metrics
        assert "lane_push_oldest_pending_age_seconds 0" in metrics
        print("PASS real binary: migrations, Connect typed operations/replay/conflicts, control receipts, MCP audience separation, Active/revocation gating, metrics")
    finally:
        process.terminate()
        process.wait(timeout=15)
        mock.shutdown()
