// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include "process.h"
#include <cassert>
#include <iostream>
#include <iterator>

using namespace lane::windows;
int wmain(int count, wchar_t** args) {
  ApartmentScope apartment;
  if (count > 1) {
    if (std::wstring(args[1]) == L"--hang") { Sleep(INFINITE); return 0; }
    if (std::wstring(args[1]) == L"--invalid") return 2;
    if (std::wstring(args[1]) == L"--error") {
      std::cout << Failure("ERROR_REASON_UNAUTHENTICATED", "Sign in again."); return 1;
    }
    if (std::wstring(args[1]) == L"--echo") {
      const std::string input(std::istreambuf_iterator<char>(std::cin), std::istreambuf_iterator<char>{});
      Json envelope, result; Set(result, L"input", input);
      envelope.Insert(L"ok", Value::CreateBooleanValue(true)); envelope.Insert(L"result", result);
      std::cout << Dump(envelope); return 0;
    }
    return 2;
  }
  assert(HTTPS("https://example.com/a?b=c#fragment"));
  for (const auto& invalid : {"http://example.com", "javascript:alert(1)", "file:///tmp/a", "https://u:p@example.com", "https://", "https://example.com\n"}) assert(!HTTPS(invalid));
  assert(!HTTPS(std::string("https://example.com\0evil", 24)));
  auto error = Parse(Failure("ERROR_REASON_INVALID_INPUT", "Invalid request."));
  assert(lane::windows::UUID(Text(error.GetNamedObject(L"error"), L"correlationId")));
  auto exact = Parse(R"({"sequence":"18446744073709551615","permission":"NOTIFICATION_PERMISSION_DENIED"})");
  assert(Text(Parse(Dump(exact)), L"sequence") == "18446744073709551615");
  // Regression evidence for the original DisconnectSync failure: the two-argument
  // GetNamedString overload throws for a present numeric value, so Text() must not
  // read the numeric retention that Settings sends.
  auto numeric = Parse(R"({"retention":2})");
  bool numeric_text_threw = false;
  try { (void)Text(numeric, L"retention"); } catch (const winrt::hresult_error&) { numeric_text_threw = true; }
  assert(numeric_text_threw);
  const auto retention = [](const char* request) { return RetentionChoice(Parse(request)); };
  assert(retention(R"({"retention":1})") == LocalRetention::KeepLocal);
  assert(retention(R"({"retention":2})") == LocalRetention::RemoveLocal);
  assert(retention(R"({"retention":"LOCAL_RETENTION_KEEP_LOCAL"})") == LocalRetention::KeepLocal);
  assert(retention(R"({"retention":"LOCAL_RETENTION_REMOVE_LOCAL"})") == LocalRetention::RemoveLocal);
  for (const auto* invalid : {"{}", R"({"retention":0})", R"({"retention":3})", R"({"retention":"INVALID"})",
                              R"({"retention":true})", R"({"retention":null})", R"({"retention":[2]})",
                              R"({"retention":{"value":2}})"})
    assert(retention(invalid) == LocalRetention::Unspecified);
  const auto input = [](const char* value) { return RetentionChoiceOfInput(Parse(value)); };
  assert(input(R"({"spaceId":"s","request":{"retention":2}})") == LocalRetention::RemoveLocal);
  assert(input(R"({"spaceId":"s","request":{"retention":1}})") == LocalRetention::KeepLocal);
  assert(input(R"({"spaceId":"s","request":"LOCAL_RETENTION_REMOVE_LOCAL"})") == LocalRetention::Unspecified);
  assert(input(R"({"spaceId":"s"})") == LocalRetention::Unspecified);
  wchar_t path[32768];
  const auto length = GetModuleFileNameW(nullptr, path, 32768);
  assert(length && length < 32768);
  // Large bidirectional traffic must not deadlock the anonymous pipes.
  const std::string input(256 * 1024, 'x');
  auto echo = Parse(RunHelper(path, {L"--echo"}, input));
  assert(echo.GetNamedBoolean(L"ok"));
  assert(Text(echo.GetNamedObject(L"result"), L"input") == input);
  auto auth = Parse(RunHelper(path, {L"--error"}));
  assert(Text(auth.GetNamedObject(L"error"), L"reason") == "ERROR_REASON_UNAUTHENTICATED");
  auto invalid = Parse(RunHelper(path, {L"--invalid"}));
  assert(!invalid.GetNamedBoolean(L"ok"));
  auto timeout = Parse(RunHelper(path, {L"--hang"}, {}, 100));
  assert(!timeout.GetNamedBoolean(L"ok"));
  auto null = NullHandle();
  auto child = Launch(path, {L"--hang"}, null.Get(), null.Get(), null.Get());
  assert(child->Running());
  child->Stop(); assert(!child->Running());
  std::cout << "Windows platform and helper transport tests passed\n";
}
