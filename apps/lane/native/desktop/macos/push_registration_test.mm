// SPDX-License-Identifier: Apache-2.0
// Exercises APNs token retention in the real native runtime against a synthetic
// helper and a scratch store. run_push_registration_test.py starts one process per
// scenario. This is not OS token delivery, APNs entitlement or provider acceptance.
#import "runtime.h"
#include "../../store/lane_store.h"
#include "../../store/vendor/json/json.hpp"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <unistd.h>
using J = nlohmann::json;
namespace fs = std::filesystem;
static void Check(bool value, const std::string& message) { if (!value) throw std::runtime_error(message); }

namespace {
const char* kEpoch = "01900000-0000-7000-8000-000000000012";
// Offset in seconds added to the fixed fixture instant; the runtime reads it as its clock.
double gClockOffset = 0;
NSData* Token(std::vector<unsigned char> bytes) { return [NSData dataWithBytes:bytes.data() length:bytes.size()]; }

struct World {
  std::string root = getenv("LANE_NATIVE_TEST_ROOT"), control = getenv("LANE_PUSH_TEST_CONTROL");
  std::vector<std::string> spaces;
  LaneRuntime* runtime = nil;
  std::shared_ptr<lane::desktop::Handler> store;

  static J Identity(int n) {
    return {{"issuer","https://fixture.invalid"},{"accountId","fixture-account-" + std::to_string(n)},
            {"tenantId","fixture-tenant"},{"epoch",kEpoch}};
  }
  static std::string Invoke(lane::Store& s, const std::string& method, const std::string& space, J request) {
    return s.invoke(method, J{{"spaceId",space},{"request",request}}.dump());
  }
  // Seeds retained account spaces. Each starts connected; `notifications` and
  // `connected` describe the state the runtime then observes.
  void Seed(const std::vector<std::pair<bool,bool>>& accounts /* {notifications, connected} */) {
    lane::Store seed(root);
    lane::HostHooks hooks;
    hooks.requestNotificationPermission = [] { return std::string("NOTIFICATION_PERMISSION_GRANTED"); };
    hooks.disconnect = [](std::string) {};
    seed.setHostHooks(hooks);
    for (size_t i = 0; i < accounts.size(); ++i) {
      auto approved = J::parse(seed.approveAccount(Identity(int(i)).dump(), false));
      Check(approved.value("ok", false), "Seed account");
      auto space = approved["response"]["spaceId"].get<std::string>();
      spaces.push_back(space);
      Check(J::parse(seed.open(J{{"formatVersion",3},{"spaceId",space}}.dump())).value("ok", false), "Seed open");
      if (accounts[i].first)
        Check(J::parse(Invoke(seed, "SetNotificationPreferences", space,
          {{"enabled",true},{"content","NOTIFICATION_CONTENT_DETAIL"}})).value("ok", false), "Seed preferences");
      if (!accounts[i].second)
        Check(J::parse(Invoke(seed, "DisconnectSync", space, {{"retention","LOCAL_RETENTION_KEEP_LOCAL"}})).value("ok", false), "Seed disconnect");
      seed.close(space);
    }
  }
  void Reapprove(int n) {
    lane::Store s(root);
    Check(J::parse(s.approveAccount(Identity(n).dump(), false)).value("ok", false), "Reapprove");
  }
  void Start() {
    runtime = [[LaneRuntime alloc] initWithHelperURL:[NSURL fileURLWithPath:[NSString stringWithUTF8String:getenv("LANE_PUSH_TEST_HELPER")]] scheme:@"lane-dev"];
    [runtime setPushClock:^NSDate*{ return [NSDate dateWithTimeIntervalSince1970:1.8e9 + gClockOffset]; } retryDelay:30];
    store = [runtime storeHandler];
    for (auto& space : spaces)
      Check(J::parse((*store)("open",{J{{"formatVersion",3},{"spaceId",space}}.dump()})).value("ok", false), "Open");
  }
  J Invoke(const std::string& method, const std::string& space, J request = J::object()) {
    return J::parse((*store)("invoke",{method,J{{"spaceId",space},{"request",request}}.dump()}));
  }
  // Every store call is a serial-queue barrier and an observation tick.
  J Status(int index) { return Invoke("GetLocalStatus", spaces[index])["response"]; }
  void Set(const std::string& name, const std::string& text) { std::ofstream(control + "/" + name) << text; }
  void Clear(const std::string& name) { std::error_code error; fs::remove(control + "/" + name, error); }
  std::vector<J> Calls() {
    std::vector<J> calls; std::ifstream in(control + "/calls"); std::string line;
    while (std::getline(in, line)) if (!line.empty()) calls.push_back(J::parse(line));
    return calls;
  }
  size_t CallsFor(int account) {
    size_t count = 0;
    for (auto& call : Calls()) if (call["expectedSpace"] == spaces[account]) ++count;
    return count;
  }
  std::vector<std::string> Workers() {
    std::vector<std::string> workers; std::ifstream in(control + "/workers"); std::string line;
    while (std::getline(in, line)) workers.push_back(line);
    return workers;
  }
  void Observe(int times = 3) {
    for (int i = 0; i < times; ++i) { [runtime waitForPushIdle:10]; Status(0); }
    Check([runtime waitForPushIdle:10], "Registration settled");
  }
  // Lets the real 500 ms timer observe repeatedly without changing the clock.
  void Quiet(double seconds = 1.3) { usleep(useconds_t(seconds * 1e6)); Observe(); }
  void WaitForCalls(size_t count) {
    for (int i = 0; i < 200 && Calls().size() < count; ++i) { Status(0); usleep(25000); }
    Check(Calls().size() >= count, "Expected helper call " + std::to_string(count));
    Observe();
  }
  void Advance(double seconds) { gClockOffset += seconds; Observe(); }
  std::string Logs() {
    std::string all;
    std::error_code error;
    for (auto& entry : fs::recursive_directory_iterator(fs::path(root) / "logs", error)) {
      if (!entry.is_regular_file(error)) continue;
      std::ifstream in(entry.path());
      all.append(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
    }
    return all;
  }
  void ExpectCall(size_t index, const std::string& endpoint, int account) {
    auto calls = Calls();
    Check(index < calls.size(), "Call exists");
    auto& call = calls[index];
    auto status = Status(account);
    Check(call["expectedSpace"] == spaces[account], "Call targets its own space");
    Check(call["request"]["endpoint"] == endpoint, "Call uses the expected token");
    Check(call["request"]["deviceId"] == status["deviceId"], "Call uses the space's device");
    Check(call["request"]["provider"] == "PUSH_PROVIDER_APNS", "APNs provider");
    Check(call["request"]["enabled"] == true, "Enabled registration");
    Check(call["request"]["content"] == "NOTIFICATION_CONTENT_DETAIL", "Current content preference");
  }
};

void RetainedAccount(World& w) {
  w.Seed({{true,false}}); w.Start();
  auto status = w.Status(0);
  Check(status["syncEnabled"] == false && status["notificationPreferences"]["enabled"] == true, "Retained and disabled for sync");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.Quiet();
  Check(w.Calls().empty(), "Disconnected account registers nothing");
  w.Reapprove(0); w.Set("space", w.spaces[0]);
  [w.runtime handleURL:[NSURL URLWithString:@"lane-dev://auth/callback?code=fixture&state=fixture"]];
  w.WaitForCalls(1);
  w.Quiet();
  Check(w.Calls().size() == 1, "One registration after reconnection, without another token callback");
  w.ExpectCall(0, "deadbeef", 0);
  Check(w.Status(0)["notificationPreferences"]["permission"] == "NOTIFICATION_PERMISSION_GRANTED" && w.Status(0)["notificationPreferences"]["enabled"] == true,
    "Preferences and permission are unchanged");
  w.Advance(1000); w.Quiet();
  Check(w.Calls().size() == 1, "No repeat after success");
}

void TransientFailure(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "transient");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Quiet();
  Check(w.Calls().size() == 1, "Failure is not retried on the observation cadence");
  auto logs = w.Logs();
  Check(logs.find("lane_push_registration_unavailable") != std::string::npos, "Failure is logged");
  Check(logs.find("deadbeef") == std::string::npos, "Log never contains token bytes");
  w.Clear("fail"); w.Advance(29); w.Quiet(0.6);
  Check(w.Calls().size() == 1, "No retry before the delay");
  w.Advance(2);
  w.WaitForCalls(2);
  Check(w.Calls().size() == 2, "One retry after the delay");
  w.ExpectCall(1, "deadbeef", 0);
  w.Advance(1000); w.Quiet();
  Check(w.Calls().size() == 2, "No call after success");
  Check(w.Logs().find("deadbeef") == std::string::npos, "Log never contains token bytes");
}

void AuthenticationFailure(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "auth");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Clear("fail"); w.Advance(3600); w.Quiet();
  Check(w.Calls().size() == 1, "Authentication failure waits for reconnection, not a timer");
  w.Set("space", w.spaces[0]);
  [w.runtime handleURL:[NSURL URLWithString:@"lane-dev://auth/callback?code=fixture&state=fixture"]];
  w.WaitForCalls(2);
  w.ExpectCall(1, "deadbeef", 0);
}

// The helper rejects a space that is not the registry's active selection with FORBIDDEN
// before any RPC. That is not an authentication failure: the space keeps retrying on the
// delay and registers once it becomes the active selection, with no reconnection event.
void SelectionMismatch(World& w) {
  w.Seed({{true,true},{true,true}}); w.Start();
  w.Set("active", w.spaces[0]);
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(2);
  w.Quiet();
  Check(w.CallsFor(0) == 1 && w.CallsFor(1) == 1, "The active space registers and the other is rejected once");
  w.Advance(31);
  for (int i = 0; i < 200 && w.CallsFor(1) < 2; ++i) { w.Status(0); usleep(25000); }
  w.Quiet();
  Check(w.CallsFor(1) == 2, "A selection mismatch retries on the delay instead of blocking");
  Check(w.CallsFor(0) == 1, "The registered active space is not repeated");
  w.Set("active", w.spaces[1]);
  w.Advance(31);
  for (int i = 0; i < 200 && w.CallsFor(1) < 3; ++i) { w.Status(0); usleep(25000); }
  w.Quiet();
  Check(w.CallsFor(1) == 3, "The space registers once it becomes the active selection");
  w.Advance(1000); w.Quiet();
  Check(w.CallsFor(0) == 1 && w.CallsFor(1) == 3, "No repeat after success");
  auto calls = w.Calls();
  Check(calls.back()["expectedSpace"] == w.spaces[1] && calls.back()["request"]["endpoint"] == "deadbeef", "Final call registers the token for its own space");
}

// A verified reconnection invalidates an attempt that was already outstanding, so a late
// authentication failure from before the reconnection cannot block registration again.
void ReconnectDuringAttempt(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "auth"); w.Set("delay", "2");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  for (int i = 0; i < 200 && w.Calls().empty(); ++i) usleep(25000);
  Check(w.Calls().size() == 1, "First attempt is in flight");
  w.Clear("fail"); w.Clear("delay");
  w.Set("space", w.spaces[0]);
  [w.runtime handleURL:[NSURL URLWithString:@"lane-dev://auth/callback?code=fixture&state=fixture"]];
  // The pre-reconnection attempt fails with authentication after the reset.
  w.WaitForCalls(2);
  w.Quiet();
  Check(w.Calls().size() == 2, "Registration continues after the late failure");
  w.ExpectCall(1, "deadbeef", 0);
  w.Advance(1000); w.Quiet();
  Check(w.Calls().size() == 2, "No call after success");
}

void HealthyCounterfactual(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Quiet(0.6);
  Check(w.Calls().empty(), "Nothing registers without a token");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Quiet();
  w.ExpectCall(0, "deadbeef", 0);
  w.Advance(100); w.Quiet();
  Check(w.Calls().size() == 1, "Repeated observation causes no extra call");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.Observe();
  Check(w.Calls().size() == 1, "The same token callback does not register again");
}

void TokenChange(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "transient");
  [w.runtime registerPushToken:Token({0xaa,0xbb,0xcc,0xdd})];
  w.WaitForCalls(1);
  [w.runtime registerPushToken:Token({0x01,0x02,0x03,0x04})];
  w.WaitForCalls(2);
  w.ExpectCall(0, "aabbccdd", 0); w.ExpectCall(1, "01020304", 0);
  w.Clear("fail"); w.Advance(31);
  w.WaitForCalls(3);
  w.ExpectCall(2, "01020304", 0);
  w.Advance(1000); w.Quiet();
  Check(w.Calls().size() == 3, "Latest token registered once");
}

void InFlightSupersession(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "transient"); w.Set("delay", "1.5");
  [w.runtime registerPushToken:Token({0xaa,0xbb,0xcc,0xdd})];
  for (int i = 0; i < 200 && w.Calls().empty(); ++i) usleep(25000);
  Check(w.Calls().size() == 1, "First attempt is in flight");
  w.Clear("fail"); w.Clear("delay");
  [w.runtime registerPushToken:Token({0x01,0x02,0x03,0x04})];
  w.Status(0);
  Check(w.Calls().size() == 1, "A newer token waits for the in-flight call");
  // The failed completion for the older token must neither retry it nor delay the newer one,
  // and the clock never advances.
  w.WaitForCalls(2);
  w.Quiet();
  Check(w.Calls().size() == 2, "Older completion cannot clear or delay the newer registration");
  w.ExpectCall(1, "01020304", 0);
}

// Fails once, then verifies a condition that makes the pending retry unsafe.
void PendingThen(World& w, const std::function<void()>& change, const std::function<void()>& after = nullptr) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "transient");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Clear("fail");
  change();
  w.Advance(31); w.Quiet();
  Check(w.Calls().size() == 1, "No registration issued from a pending attempt");
  if (after) after();
}

void DisconnectBeforeRetry(World& w) {
  PendingThen(w, [&] { Check(w.Invoke("DisconnectSync", w.spaces[0], {{"retention","LOCAL_RETENTION_KEEP_LOCAL"}}).value("ok", false), "Disconnect"); }, [&] {
    w.Reapprove(0); w.Set("space", w.spaces[0]);
    [w.runtime handleURL:[NSURL URLWithString:@"lane-dev://auth/callback?code=fixture&state=fixture"]];
    w.WaitForCalls(2);
    w.ExpectCall(1, "deadbeef", 0);
    w.Quiet();
    Check(w.Calls().size() == 2, "Reconnection registers once");
  });
}

void DisableBeforeRetry(World& w) {
  PendingThen(w, [&] { Check(w.Invoke("SetNotificationPreferences", w.spaces[0], {{"enabled",false},{"content","NOTIFICATION_CONTENT_DETAIL"}}).value("ok", false), "Disable"); });
}

void RemoveBeforeRetry(World& w) {
  PendingThen(w, [&] { Check(w.Invoke("RemoveDeviceData", w.spaces[0]).value("ok", false), "Remove"); });
}

void StopBeforeRetry(World& w) {
  w.Seed({{true,true}}); w.Start();
  w.Set("fail", "transient");
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Clear("fail");
  [w.runtime stop];
  usleep(300000);
  gClockOffset += 31;
  usleep(1300000);
  Check(w.Calls().size() == 1, "A stopped runtime registers nothing");
  Check([w.runtime waitForPushIdle:5], "Idle");
}

void AccountIsolation(World& w) {
  w.Seed({{true,true},{false,false}}); w.Start();
  [w.runtime registerPushToken:Token({0xde,0xad,0xbe,0xef})];
  w.WaitForCalls(1);
  w.Quiet();
  Check(w.Calls().size() == 1, "Only the enabled account registers");
  w.ExpectCall(0, "deadbeef", 0);
  // A remote proposal wake for the disabled retained account starts and registers nothing for it.
  [w.runtime receivePush:@{@"type":@"lane.proposal",@"accountId":@"fixture-account-1",@"proposalId":@"01900000-0000-7000-8000-000000000099"}];
  w.Quiet();
  for (auto& space : w.Workers()) Check(space != w.spaces[1], "Wake does not start the disabled account's worker");
  Check(w.Calls().size() == 1, "Wake does not register");
  [w.runtime receivePush:@{@"type":@"lane.proposal",@"accountId":@"fixture-account-0",@"proposalId":@"01900000-0000-7000-8000-000000000099"}];
  w.Quiet();
  Check(!w.Workers().empty() && w.Workers().front() == w.spaces[0], "Wake dispatches to its own account");
  Check(w.Calls().size() == 1, "Wake does not repeat registration");
  // Reconnecting the retained account with notifications still disabled stays untouched.
  w.Reapprove(1); w.Quiet();
  Check(w.Calls().size() == 1, "A reconnected account with notifications off is untouched");
  // Disconnect and reconnect the enabled account: it registers again for its own space.
  Check(w.Invoke("DisconnectSync", w.spaces[0], {{"retention","LOCAL_RETENTION_KEEP_LOCAL"}}).value("ok", false), "Disconnect");
  w.Quiet(0.6);
  Check(w.Calls().size() == 1, "Disconnected account does not register");
  w.Reapprove(0); w.Quiet();
  w.WaitForCalls(2);
  w.ExpectCall(1, "deadbeef", 0);
  for (auto& call : w.Calls()) Check(call["expectedSpace"] != w.spaces[1], "Never registered under the other account");
}
}

int main(int argc, char** argv) {
  @autoreleasepool {
    try {
      Check(argc == 2 && getenv("LANE_NATIVE_TEST_ROOT") && getenv("LANE_PUSH_TEST_CONTROL") && getenv("LANE_PUSH_TEST_HELPER"), "Scenario and scratch environment required");
      World world;
      std::string name = argv[1];
      struct Stop {
        World& world;
        // Stop the runtime and wait for its storage queue so synthetic worker helpers exit with the test.
        ~Stop() { if (world.runtime) { [world.runtime stop]; [world.runtime waitForPushIdle:5]; } }
      } stop{world};
      if (name == "retained-account") RetainedAccount(world);
      else if (name == "transient-failure") TransientFailure(world);
      else if (name == "authentication-failure") AuthenticationFailure(world);
      else if (name == "selection-mismatch") SelectionMismatch(world);
      else if (name == "reconnect-during-attempt") ReconnectDuringAttempt(world);
      else if (name == "healthy-counterfactual") HealthyCounterfactual(world);
      else if (name == "token-change") TokenChange(world);
      else if (name == "in-flight-supersession") InFlightSupersession(world);
      else if (name == "disconnect-before-retry") DisconnectBeforeRetry(world);
      else if (name == "disable-before-retry") DisableBeforeRetry(world);
      else if (name == "remove-before-retry") RemoveBeforeRetry(world);
      else if (name == "stop-before-retry") StopBeforeRetry(world);
      else if (name == "account-isolation") AccountIsolation(world);
      else throw std::runtime_error("Unknown scenario");
      std::cout << "native push registration scenario passed: " << name << "\n";
    } catch (const std::exception& error) {
      std::cerr << "native push registration scenario failed: " << error.what() << "\n";
      return 1;
    }
  }
  return 0;
}
