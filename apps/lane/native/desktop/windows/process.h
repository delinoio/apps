// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "platform.h"
#include <map>
#include <thread>

namespace lane::windows {
struct Process {
  Handle job, process;
  DWORD id = 0;
  bool Running() const { return process && WaitForSingleObject(process.Get(), 0) == WAIT_TIMEOUT; }
  DWORD ExitCode() const {
    DWORD result = 0;
    if (!process || !GetExitCodeProcess(process.Get(), &result)) return ERROR_PROCESS_ABORTED;
    return result;
  }
  void Stop() {
    // Termination cannot strand a SQL transaction: SQLite rolls it back and
    // the shared sync lease expires. Never kill another CLI owner's process.
    if (Running()) TerminateJobObject(job.Get(), ERROR_CANCELLED);
    if (process) WaitForSingleObject(process.Get(), 5000);
    process.Reset(); job.Reset();
  }
  ~Process() { Stop(); }
};
inline std::unique_ptr<Process> Launch(const std::filesystem::path& executable,
    const std::vector<std::wstring>& args, HANDLE input, HANDLE output, HANDLE error) {
  if (!std::filesystem::is_regular_file(executable)) throw std::runtime_error("Packaged helper is missing");
  std::wstring command = QuoteArgument(executable.wstring());
  for (const auto& arg : args) { command += L' '; command += QuoteArgument(arg); }
  auto child = std::make_unique<Process>();
  child->job.Reset(CreateJobObjectW(nullptr, nullptr));
  if (!child->job) throw std::runtime_error("Cannot create helper job");
  JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
  limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
  if (!SetInformationJobObject(child->job.Get(), JobObjectExtendedLimitInformation, &limits, sizeof(limits)))
    throw std::runtime_error("Cannot configure helper job");
  SIZE_T bytes = 0;
  InitializeProcThreadAttributeList(nullptr, 1, 0, &bytes);
  std::vector<unsigned char> memory(bytes);
  auto attributes = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(memory.data());
  if (!InitializeProcThreadAttributeList(attributes, 1, 0, &bytes)) throw std::runtime_error("Cannot initialize helper handles");
  struct Cleanup { LPPROC_THREAD_ATTRIBUTE_LIST value; ~Cleanup() { DeleteProcThreadAttributeList(value); } } cleanup{attributes};
  // The explicit handle list prevents an overlapping helper launch from
  // inheriting another operation's pipes, database handles or credentials.
  std::vector<HANDLE> handles{input, output, error};
  std::sort(handles.begin(), handles.end());
  handles.erase(std::unique(handles.begin(), handles.end()), handles.end());
  if (!UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
      handles.data(), handles.size() * sizeof(HANDLE), nullptr, nullptr)) throw std::runtime_error("Cannot isolate helper handles");
  STARTUPINFOEXW startup{};
  startup.StartupInfo.cb = sizeof(startup);
  startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
  startup.StartupInfo.hStdInput = input;
  startup.StartupInfo.hStdOutput = output;
  startup.StartupInfo.hStdError = error;
  startup.lpAttributeList = attributes;
  PROCESS_INFORMATION info{};
  if (!CreateProcessW(executable.c_str(), command.data(), nullptr, nullptr, TRUE,
      CREATE_NO_WINDOW | CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
      nullptr, executable.parent_path().c_str(), &startup.StartupInfo, &info)) throw std::runtime_error("Cannot launch packaged helper");
  child->process.Reset(info.hProcess);
  Handle thread(info.hThread);
  child->id = info.dwProcessId;
  if (!AssignProcessToJobObject(child->job.Get(), child->process.Get())) {
    TerminateProcess(child->process.Get(), ERROR_CANCELLED);
    throw std::runtime_error("Cannot supervise packaged helper");
  }
  if (ResumeThread(thread.Get()) == static_cast<DWORD>(-1)) throw std::runtime_error("Cannot start packaged helper");
  return child;
}
inline Handle NullHandle() {
  SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
  Handle result(CreateFileW(L"NUL", GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
      &security, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
  if (!result) throw std::runtime_error("Cannot open helper standard handles");
  return result;
}
inline std::string RunHelper(const std::filesystem::path& executable,
    const std::vector<std::wstring>& args, const std::string& input = {}, DWORD timeout_ms = 60000) {
  SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
  HANDLE read = nullptr, write = nullptr;
  if (!CreatePipe(&read, &write, &security, 0)) throw std::runtime_error("Cannot create helper input");
  Handle child_input(read), parent_input(write);
  if (!SetHandleInformation(parent_input.Get(), HANDLE_FLAG_INHERIT, 0)) throw std::runtime_error("Cannot isolate helper input");
  if (!CreatePipe(&read, &write, &security, 0)) throw std::runtime_error("Cannot create helper output");
  Handle parent_output(read), child_output(write);
  if (!SetHandleInformation(parent_output.Get(), HANDLE_FLAG_INHERIT, 0)) throw std::runtime_error("Cannot isolate helper output");
  auto error = NullHandle();
  auto child = Launch(executable, args, child_input.Get(), child_output.Get(), error.Get());
  child_input.Reset(); child_output.Reset();
  std::string output;
  bool read_ok = true, write_ok = true;
  std::thread reader([&] {
    char buffer[8192]; DWORD count;
    for (;;) {
      if (!ReadFile(parent_output.Get(), buffer, sizeof(buffer), &count, nullptr)) {
        read_ok = GetLastError() == ERROR_BROKEN_PIPE; break;
      }
      if (!count) break;
      try { output.append(buffer, count); } catch (...) { read_ok = false; break; }
    }
  });
  std::thread writer([&] {
    size_t position = 0;
    while (position < input.size()) {
      DWORD written = 0;
      if (!WriteFile(parent_input.Get(), input.data() + position,
          static_cast<DWORD>(std::min<size_t>(input.size() - position, 65536)), &written, nullptr) || !written) { write_ok = false; break; }
      position += written;
    }
    parent_input.Reset();
  });
  const bool timeout = WaitForSingleObject(child->process.Get(), timeout_ms) != WAIT_OBJECT_0;
  const DWORD code = timeout ? ERROR_TIMEOUT : child->ExitCode();
  if (timeout) child->Stop();
  writer.join(); reader.join();
  if (timeout) return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "The Lane helper timed out. Retry the operation.", true);
  try {
    auto response = Parse(output);
    if (read_ok && response.HasKey(L"ok") && (!response.GetNamedBoolean(L"ok") ||
        (write_ok && code == 0 && response.HasKey(L"result")))) return Dump(response);
  } catch (...) {}
  if (!read_ok || !write_ok) return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "The Lane helper transport failed.", true);
  return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE",
      code == 2 ? "The packaged Lane helper does not support this operation. Update Lane." : "The Lane helper failed without a valid response.", code != 2);
}
class SyncSupervisor {
 public:
  using Report = std::function<void(const std::string&, SyncState, DWORD)>;
  SyncSupervisor(std::filesystem::path helper, Report report) : helper_(std::move(helper)), report_(std::move(report)) {}
  void Reconcile(const std::string& space, bool enabled) {
    if (!UUID(space)) return;
    auto& entry = entries_[space];
    if (!enabled) { Stop(space); return; }
    if (entry.process && entry.process->Running()) {
      if (std::chrono::steady_clock::now() - entry.started > std::chrono::minutes(1)) entry.failures = 0;
      return;
    }
    if (entry.process) {
      const auto exit = entry.process->ExitCode();
      entry.process.reset();
      Change(space, entry, exit == 2 ? SyncState::InvalidInvocation : SyncState::Exited, exit);
      entry.retry = std::chrono::steady_clock::now() + RetryDelay(entry.failures++);
    }
    if (std::chrono::steady_clock::now() < entry.retry) return;
    try {
      if (!std::filesystem::is_regular_file(helper_)) {
        Change(space, entry, SyncState::MissingHelper, ERROR_FILE_NOT_FOUND);
      } else {
        Change(space, entry, SyncState::Starting, 0);
        auto null = NullHandle();
        entry.process = Launch(helper_, {L"--space", winrt::to_hstring(space).c_str(), L"sync", L"run"}, null.Get(), null.Get(), null.Get());
        entry.started = std::chrono::steady_clock::now();
        // Running means the process exists, never that it authenticated or synced.
        Change(space, entry, SyncState::Running, 0);
        return;
      }
    } catch (...) { Change(space, entry, SyncState::LaunchFailed, GetLastError()); }
    entry.retry = std::chrono::steady_clock::now() + RetryDelay(entry.failures++);
  }
  void Stop(const std::string& space) {
    const auto found = entries_.find(space);
    if (found == entries_.end()) return;
    auto& entry = found->second;
    if (entry.process) { Change(space, entry, SyncState::Stopping, 0); entry.process.reset(); }
    Change(space, entry, SyncState::Disabled, 0);
    entry.retry = {}; entry.failures = 0;
  }
 private:
  struct Entry {
    std::unique_ptr<Process> process;
    SyncState state = SyncState::Disabled;
    unsigned failures = 0;
    std::chrono::steady_clock::time_point retry{}, started{};
  };
  void Change(const std::string& space, Entry& entry, SyncState state, DWORD code) {
    if (entry.state != state) { entry.state = state; report_(space, state, code); }
  }
  std::filesystem::path helper_;
  Report report_;
  std::map<std::string, Entry> entries_;
};
}  // namespace lane::windows
