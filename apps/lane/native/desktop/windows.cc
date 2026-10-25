// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include "lane.h"

struct LaneWindow {
  std::shared_ptr<LaneClient> client = std::make_shared<LaneClient>();
  std::unique_ptr<lynx::pub::LynxView> view;
};
LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
  auto* lane = reinterpret_cast<LaneWindow*>(GetWindowLongPtrW(window, GWLP_USERDATA));
  if (lane && lane->view) {
    if (message == WM_SIZE) lane->view->SetFrame(0, 0, LOWORD(lparam), HIWORD(lparam));
    if (message == WM_ACTIVATE) {
      if (LOWORD(wparam) == WA_INACTIVE) lane->view->OnEnterBackground();
      else lane->view->OnEnterForeground();
    }
  }
  if (message == WM_DESTROY) {
    if (lane) lane->view.reset();
    PostQuitMessage(0);
    return 0;
  }
  return DefWindowProcW(window, message, wparam, lparam);
}
int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int show) {
  wchar_t executable[32768];
  const auto length = GetModuleFileNameW(nullptr, executable, 32768);
  if (!length || length == 32768) return 1;
  const auto directory = std::filesystem::path(executable).parent_path();
  auto& env = lynx::pub::LynxEnv::GetInstance();
  const auto icu = (directory / "data" / "icudtl.dat").string();
  env.SetICUDataPath(icu.c_str());
  WNDCLASSW type{};
  type.lpfnWndProc = WindowProc;
  type.hInstance = instance;
  type.lpszClassName = L"LaneWindow";
  type.hCursor = LoadCursor(nullptr, IDC_ARROW);
  if (!RegisterClassW(&type)) return 1;
  HWND window = CreateWindowExW(0, type.lpszClassName, L"Lane", WS_OVERLAPPEDWINDOW,
    CW_USEDEFAULT, CW_USEDEFAULT, 1280, 820, nullptr, nullptr, instance, nullptr);
  if (!window) return 1;
  LaneWindow lane;
  SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(&lane));
  RECT bounds{};
  GetClientRect(window, &bounds);
  lynx::pub::LynxView::Builder builder;
  builder.SetScreenSize(bounds.right, bounds.bottom, GetDpiForWindow(window) / 96.0f)
    .SetFrame(0, 0, bounds.right, bounds.bottom).SetParent(window);
  lane.view = builder.Build();
  lane.view->AddClient(lane.client);
  try { LoadLane(*lane.view, directory / "main.lynx.bundle"); }
  catch (const std::exception&) {
    std::cerr << "{\"event\":\"lane_start_failed\"}\n";
    DestroyWindow(window);
    return 1;
  }
  ShowWindow(window, show);
  std::clog << "{\"event\":\"lane_started\",\"platform\":\"windows\"}\n";
  MSG message{};
  int status;
  while ((status = GetMessageW(&message, nullptr, 0, 0)) > 0) {
    TranslateMessage(&message);
    DispatchMessageW(&message);
  }
  return status < 0 ? 1 : static_cast<int>(message.wParam);
}
