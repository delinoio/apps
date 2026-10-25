// SPDX-License-Identifier: Apache-2.0
// Run from a scratch directory with helpers/lane set to reauth_helper.py.
// This exercises the real native runtime against a synthetic helper, not OAuth.
#import "runtime.h"
#include "../../store/lane_store.h"
#include "../../store/vendor/json/json.hpp"
#include <filesystem>
#include <fstream>
#include <signal.h>
#include <unistd.h>
#include <iostream>
#include <stdexcept>
using J = nlohmann::json;
static void Check(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
int main() {
  @autoreleasepool {
    const char* root = getenv("LANE_NATIVE_TEST_ROOT");
    Check(root != nullptr, "Explicit scratch root required");
    const std::string account = "01900000-0000-7000-8000-000000000010";
    const std::string challenge = "01900000-0000-7000-8000-000000000020";
    std::string space;
    {
      lane::Store seed(root);
      auto approved = J::parse(seed.approveAccount(J{{"issuer","https://test.invalid"},
        {"accountId",account},{"tenantId","01900000-0000-7000-8000-000000000011"},
        {"epoch","01900000-0000-7000-8000-000000000012"}}.dump(),false));
      Check(approved.value("ok",false), "Seed account");
      space = J::parse(seed.open(R"({"formatVersion":1,"spaceId":""})"))["response"]["spaceId"];
    }
    LaneRuntime* runtime = [LaneRuntime new];
    auto store = [runtime storeHandler], host = [runtime hostHandler];
    Check(J::parse((*store)("open",{R"({"formatVersion":1,"spaceId":""})"})).value("ok",false), "Open");
    auto workerFile = std::filesystem::path(root)/"helper-worker-pid";
    for (int attempt=0; attempt<30 && !std::filesystem::exists(workerFile); ++attempt) usleep(100000);
    int workerPID = 0; std::ifstream(workerFile) >> workerPID;
    Check(workerPID > 0 && kill(workerPID,0) == 0, "Background helper started");
    auto invoke = [&](std::string method, J request) {
      return J::parse((*store)("invoke",{method,J{{"spaceId",space},{"request",request}}.dump()}));
    };
    J request = {{"confirmedAccountId",account},{"mode","DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"}};
    auto begin = [&] { Check(invoke("BeginDataDeletion",request).value("ok",false), "Begin"); };
    auto callback = [&] {
      [runtime handleURL:[NSURL URLWithString:[NSString stringWithFormat:@"lane-dev://auth/callback?reauth_challenge=%s",challenge.c_str()]]];
      invoke("GetLocalStatus",J::object()); // Serial worker barrier.
    };
    auto deletion = request; deletion["challengeId"] = challenge;
    begin();
    Check(!invoke("DeleteAccountData",deletion).value("ok",true), "Delete before callback rejected");
    callback();
    Check(!std::filesystem::exists(std::filesystem::path(root)/"helper-deleted"), "Callback must not delete");
    auto wrong = deletion; wrong["challengeId"] = "01900000-0000-7000-8000-000000000021";
    Check(!invoke("DeleteAccountData",wrong).value("ok",true), "Wrong challenge rejected");
    wrong = deletion; wrong["mode"] = "DATA_DELETION_MODE_DELETE_EVERYWHERE";
    Check(!invoke("DeleteAccountData",wrong).value("ok",true), "Wrong mode rejected");
    wrong = deletion; wrong["confirmedAccountId"] = "01900000-0000-7000-8000-000000000022";
    Check(!invoke("DeleteAccountData",wrong).value("ok",true), "Wrong account rejected");
    wrong = deletion; wrong["reauthProof"] = "untrusted";
    Check(!invoke("DeleteAccountData",wrong).value("ok",true), "JS proof rejected");
    Check(J::parse((*host)("cancelReauthentication",{challenge})).value("ok",false), "Cancel");
    callback();
    Check(!invoke("DeleteAccountData",deletion).value("ok",true), "Canceled callback rejected");
    begin(); callback(); [runtime viewClosed]; invoke("GetLocalStatus",J::object());
    Check(kill(workerPID,0) == 0, "Window closure retains background helper");
    Check(!invoke("DeleteAccountData",deletion).value("ok",true), "Closed view rejected");
    begin(); callback();
    Check(invoke("DeleteAccountData",deletion).value("ok",false), "Explicit final deletion");
    Check(std::filesystem::exists(std::filesystem::path(root)/"helper-deleted"), "Helper final RPC executed");
    Check(!invoke("DeleteAccountData",deletion).value("ok",true), "Replay rejected");
    Check(kill(workerPID,0) != 0, "Disconnect stops the owned background helper");
    [runtime stop];
    std::cout << "native reauth helper boundary passed\n";
  }
}
