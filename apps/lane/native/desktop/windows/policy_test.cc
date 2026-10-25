// SPDX-License-Identifier: Apache-2.0
#include "policy.h"
#include "serial_worker.h"
#include <atomic>
#include <cassert>
#include <iostream>

using namespace lane::windows;
int main() {
  const std::string id = "019a1700-1234-7abc-8def-0123456789ab";
  assert(UUID(id));
  assert(!UUID("019a1700-1234-4abc-8def-0123456789ab"));
  assert(!UUID("019a1700-1234-7abc-7def-0123456789ab"));
  assert(!UUID("019A1700-1234-7abc-8def-0123456789ab"));
  Route route;
  assert(ParseRoute("lane://open", "lane", route) && route.kind == RouteKind::Open);
  assert(ParseRoute("lane-dev://", "lane-dev", route));
  assert(ParseRoute("lane://task/" + id, "lane", route) && route.id == id && route.kind == RouteKind::Task);
  assert(ParseRoute("lane://repository/" + id, "lane", route) && route.kind == RouteKind::Repository);
  assert(ParseRoute("lane://auth/callback?state=abc&code=def%20ghi", "lane", route) && route.kind == RouteKind::Callback);
  assert(ParseRoute("lane://auth/callback?error=access_denied&state=abc", "lane", route));
  for (const auto& invalid : {
      "https://open", "lane-dev://open", "lane://open#fragment", "lane://decision/x",
      "lane://task/../x", "lane://task/%30", "lane://user@open", "lane://open:90",
      "lane://auth/callback?code=a", "lane://auth/callback?state=a&state=b&code=c",
      "lane://auth/callback?state=a&code=c&error=x", "lane://auth/callback?state=a&code=",
      "lane://auth/callback?state=a&code=%x0", "lane://auth/callback?state=a&code=b&",
      "lane://auth/callback?state=a&code=b#fragment", "lane://auth/callback?state=a&token=b",
      "lane://open\n", "lane://open\\", "lane://open\""}) assert(!ParseRoute(invalid, "lane", route));
  assert(!ParseRoute("lane://task/" + id + "?spaceId=" + id, "lane", route));
  assert(!ParseRoute(std::string("lane://open\0bad", 15), "lane", route));
  assert(QuoteArgument(L"") == L"\"\"");
  assert(QuoteArgument(L"a b") == L"\"a b\"");
  assert(QuoteArgument(L"a\"b") == L"\"a\\\"b\"");
  assert(QuoteArgument(L"C:\\path with spaces\\") == L"\"C:\\path with spaces\\\\\"");
  assert(RetryDelay(0).count() == 1 && RetryDelay(1000).count() == 60);

  std::thread::id owner;
  unsigned counter = 0;
  std::atomic<bool> finished{false};
  std::vector<std::future<std::string>> results;
  SerialWorker worker([&] { owner = std::this_thread::get_id(); }, [] {}, [&] {
    assert(std::this_thread::get_id() == owner); finished = true;
  });
  for (unsigned i = 0; i != 500; ++i) {
    results.push_back(worker.Submit([&, i] {
      assert(std::this_thread::get_id() == owner);
      assert(counter++ == i);
      return std::to_string(i);
    }));
  }
  for (unsigned i = 0; i != results.size(); ++i) assert(results[i].get() == std::to_string(i));
  auto failed = worker.Submit([]() -> std::string { throw std::runtime_error("rollback"); });
  try { (void)failed.get(); assert(false); } catch (const std::runtime_error&) {}
  assert(worker.Submit([] { return std::string("still available"); }).get() == "still available");
  worker.Stop(); assert(finished);
  try { worker.Submit([] { return std::string{}; }); assert(false); } catch (const std::runtime_error&) {}

  SerialWorker startup([] { throw std::runtime_error("unsupported format"); }, [] { assert(false); }, [] {});
  try { (void)startup.Submit([] { assert(false); return std::string{}; }).get(); assert(false); }
  catch (const std::runtime_error&) {}
  startup.Stop();
  std::cout << "Windows host portable policy and serial-worker tests passed\n";
}
