// SPDX-License-Identifier: Apache-2.0
// Chrome launches only this relay and its bundled Node runtime. This executable
// never loads Lynxtron and cannot start the Lane application.
#include <filesystem>
#include <string>
#include <vector>
#include <cstdlib>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shlobj.h>
static std::wstring Quote(const std::wstring& input) {
  std::wstring result=L"\"";size_t slashes=0;
  for(wchar_t c:input){if(c==L'\\'){slashes++;continue;}if(c==L'"')result.append(slashes*2+1,L'\\');else result.append(slashes,L'\\');slashes=0;result+=c;}
  result.append(slashes*2,L'\\');return result+L"\"";
}
int wmain(int argc,wchar_t** argv) {
  wchar_t executable[32768];DWORD size=GetModuleFileNameW(nullptr,executable,32768);if(!size||size==32768)return 1;
  auto directory=std::filesystem::path(executable).parent_path();PWSTR local=nullptr;
  if(FAILED(SHGetKnownFolderPath(FOLDERID_LocalAppData,0,nullptr,&local)))return 1;
  auto root=std::filesystem::path(local)/L"io.delino.lane";CoTaskMemFree(local);
#ifdef LANE_NATIVE_TESTING
  if(const auto test=_wgetenv(L"LANE_NATIVE_TEST_ROOT");test&&std::filesystem::path(test).is_absolute())root=test;
#endif
  std::vector<std::wstring> arguments={(directory/L"node.exe").wstring(),(directory/L"chrome-native-host.mjs").wstring(),L"--data-root",root.wstring()};
  for(int i=1;i<argc;i++)arguments.emplace_back(argv[i]);
  std::wstring command;for(const auto& arg:arguments){if(!command.empty())command+=L' ';command+=Quote(arg);}
  STARTUPINFOW startup{sizeof(startup)};startup.dwFlags=STARTF_USESTDHANDLES;startup.hStdInput=GetStdHandle(STD_INPUT_HANDLE);startup.hStdOutput=GetStdHandle(STD_OUTPUT_HANDLE);startup.hStdError=GetStdHandle(STD_ERROR_HANDLE);
  PROCESS_INFORMATION process{};
  if(!CreateProcessW(arguments[0].c_str(),command.data(),nullptr,nullptr,TRUE,CREATE_NO_WINDOW,nullptr,directory.c_str(),&startup,&process))return 1;
  CloseHandle(process.hThread);WaitForSingleObject(process.hProcess,INFINITE);DWORD code=1;GetExitCodeProcess(process.hProcess,&code);CloseHandle(process.hProcess);return (int)code;
}
#else
#include <mach-o/dyld.h>
#include <unistd.h>
#include <pwd.h>
int main(int argc,char** argv) {
  uint32_t size=0;_NSGetExecutablePath(nullptr,&size);std::vector<char> executable(size);
  if(_NSGetExecutablePath(executable.data(),&size)!=0)return 1;
  auto directory=std::filesystem::canonical(executable.data()).parent_path();
  auto user=getpwuid(getuid());if(!user)return 1;
  auto root=std::filesystem::path(user->pw_dir)/"Library/Application Support/io.delino.lane";
#ifdef LANE_NATIVE_TESTING
  if(const auto test=getenv("LANE_NATIVE_TEST_ROOT");test&&std::filesystem::path(test).is_absolute())root=test;
#endif
  std::filesystem::path runtime=directory/"node";
  if(!__builtin_available(macOS 13.5,*))runtime=directory/"node-macos12";
  std::vector<std::string> strings={runtime.string(),(directory/"chrome-native-host.mjs").string(),"--data-root",root.string()};
  for(int i=1;i<argc;i++)strings.emplace_back(argv[i]);
  std::vector<char*> arguments;for(auto& item:strings)arguments.push_back(item.data());arguments.push_back(nullptr);
  execv(arguments[0],arguments.data());return 1;
}
#endif
