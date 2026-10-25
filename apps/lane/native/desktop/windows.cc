// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <sddl.h>
#include <optional>
#include "lane.h"
#include "windows/notifications.h"
#include "windows/service.h"
#include "async_module.h"

namespace {
using namespace lane::windows;
#if defined(LANE_DEVELOPMENT)
constexpr wchar_t kControllerClass[] = L"LaneDevelopmentController";
constexpr wchar_t kWindowClass[] = L"LaneDevelopmentWindow";
constexpr char kScheme[] = "lane-dev";
#else
constexpr wchar_t kControllerClass[] = L"LaneController";
constexpr wchar_t kWindowClass[] = L"LaneWindow";
constexpr char kScheme[] = "lane";
#endif
constexpr UINT kTray = WM_APP + 2;
constexpr UINT kOpen = 1001, kQuit = 1002;
constexpr ULONG_PTR kActivation = 0x4c414e45;
LRESULT CALLBACK WindowProc(HWND, UINT, WPARAM, LPARAM);

std::wstring UserSID() {
  HANDLE raw = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw)) throw std::runtime_error("Cannot identify OS user");
  Handle token(raw);
  DWORD size = 0;
  GetTokenInformation(token.Get(), TokenUser, nullptr, 0, &size);
  std::vector<unsigned char> bytes(size);
  if (!GetTokenInformation(token.Get(), TokenUser, bytes.data(), size, &size)) throw std::runtime_error("Cannot identify OS user");
  LPWSTR sid = nullptr;
  if (!ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(bytes.data())->User.Sid, &sid)) throw std::runtime_error("Cannot identify OS user");
  std::wstring result(sid); LocalFree(sid); return result;
}
void RegisterProtocol(const std::filesystem::path& executable) {
  const std::wstring base = L"Software\\Classes\\" + std::wstring(winrt::to_hstring(kScheme));
  auto set = [](const std::wstring& path, const wchar_t* name, const std::wstring& value) {
    HKEY key = nullptr;
    if (RegCreateKeyExW(HKEY_CURRENT_USER, path.c_str(), 0, nullptr, 0, KEY_SET_VALUE, nullptr, &key, nullptr) != ERROR_SUCCESS)
      throw std::runtime_error("Protocol registration failed");
    const auto result = RegSetValueExW(key, name, 0, REG_SZ,
        reinterpret_cast<const BYTE*>(value.c_str()), static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t)));
    RegCloseKey(key);
    if (result != ERROR_SUCCESS) throw std::runtime_error("Protocol registration failed");
  };
  set(base, nullptr, L"URL:Lane Protocol");
  set(base, L"URL Protocol", L"");
  set(base + L"\\shell\\open\\command", nullptr, QuoteArgument(executable.wstring()) + L" --uri \"%1\"");
  SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, nullptr, nullptr);
}
bool Forward(const std::wstring& user, const std::string& uri) {
  for (unsigned attempt = 0; attempt != 100; ++attempt) {
    HWND controller = FindWindowExW(HWND_MESSAGE, nullptr, kControllerClass, user.c_str());
    if (controller) {
      auto wide = winrt::to_hstring(uri);
      COPYDATASTRUCT data{kActivation, static_cast<DWORD>((wide.size() + 1) * sizeof(wchar_t)), const_cast<wchar_t*>(wide.c_str())};
      DWORD_PTR accepted = 0;
      return SendMessageTimeoutW(controller, WM_COPYDATA, 0, reinterpret_cast<LPARAM>(&data),
          SMTO_ABORTIFHUNG | SMTO_BLOCK, 3000, &accepted) && accepted;
    }
    Sleep(50);
  }
  return false;
}

class Application {
 public:
  Application(HINSTANCE instance, std::filesystem::path executable, std::wstring user)
      : instance_(instance), executable_(std::move(executable)), user_(std::move(user)) {}
  ~Application() { Shutdown(); }
  void Start() {
    WNDCLASSW type{};
    type.lpfnWndProc = WindowProc; type.hInstance = instance_;
    type.lpszClassName = kControllerClass;
    if (!RegisterClassW(&type)) throw std::runtime_error("Controller registration failed");
    type.lpszClassName = kWindowClass; type.hCursor = LoadCursor(nullptr, IDC_ARROW);
    type.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
    if (!RegisterClassW(&type)) throw std::runtime_error("Window registration failed");
    controller_ = CreateWindowExW(0, kControllerClass, user_.c_str(), 0, 0, 0, 0, 0,
        HWND_MESSAGE, nullptr, instance_, this);
    if (!controller_) throw std::runtime_error("Controller creation failed");
    ui_->Attach(controller_);
    RegisterProtocol(executable_);
    const auto icu = (executable_.parent_path() / "data" / "icudtl.dat").string();
    lynx::pub::LynxEnv::GetInstance().SetICUDataPath(icu.c_str());
    notifications_->Start(executable_.parent_path(), [this, ui = ui_](std::string payload) {
      ui->Post([this, payload = std::move(payload)] {
        auto value = Parse(payload);
        const auto id = Text(value, L"id"), space = Text(value, L"spaceId");
        if (Text(value, L"kind") != "task" || !UUID(id) || !UUID(space)) return;
        pending_route_ = value;
        Show(); SendRoute();
      });
    });
    HostActions actions;
    actions.event = [this, ui = ui_](std::string name, std::string argument) {
      ui->Post([this, name = std::move(name), argument = std::move(argument)] {
        if (view_ && ready_) view_->SendGlobalEvent(name, "[" + argument + "]");
      });
    };
    actions.permission = [ui = ui_, notifications = notifications_] {
      return ui->Call([notifications] { return notifications->Permission(); });
    };
    actions.notify = [ui = ui_, notifications = notifications_](Json item) {
      return ui->Call([notifications, item] { return notifications->Show(item) ? "accepted" : "unavailable"; }) == "accepted";
    };
    actions.browse = [this, ui = ui_](std::string url) {
      auto result = ui->Call([this, url] { return OpenURL(controller_, url); });
      if (!Parse(result).GetNamedBoolean(L"ok", false)) throw std::runtime_error("Browser launch failed");
    };
    // Lane.exe and lane.exe are the same filename on Windows.
    service_ = std::make_shared<Service>(executable_.parent_path() / "helpers" / "lane.exe", kScheme, std::move(actions));
    host_handler_ = std::make_shared<lane::desktop::Handler>([this, ui = ui_](const std::string& method, const auto& args) {
      return ui->Call([this, method, text = args.at(0)] {
        return method == "copyText" ? CopyText(controller_, text) : OpenURL(controller_, text);
      });
    });
    AddTray();
  }
  void Show() {
    if (window_) { ShowWindow(window_, SW_RESTORE); SetForegroundWindow(window_); return; }
    window_ = CreateWindowExW(0, kWindowClass, L"Lane", WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT, CW_USEDEFAULT, 1280, 820, nullptr, nullptr, instance_, this);
    if (!window_) throw std::runtime_error("Window creation failed");
    HMENU menu = CreateMenu(), file = CreatePopupMenu();
    AppendMenuW(file, MF_STRING, kQuit, L"&Quit Lane");
    AppendMenuW(menu, MF_POPUP, reinterpret_cast<UINT_PTR>(file), L"&File");
    SetMenu(window_, menu);
    RECT bounds{}; GetClientRect(window_, &bounds);
    ready_ = false;
    const auto generation = ++generation_;
    store_handler_ = std::make_shared<lane::desktop::Handler>([service = service_, this, ui = ui_, generation](const auto& method, const auto& args) {
      auto result = service->Call(method, args);
      if (method == "open" && Parse(result).GetNamedBoolean(L"ok", false)) {
        ui->Post([this, generation] {
          if (generation == generation_ && view_) { ready_ = true; SendRoute(); service_->Resume(); }
        });
      }
      return result;
    });
    lynx::pub::LynxView::Builder builder;
    builder.SetScreenSize(bounds.right, bounds.bottom, GetDpiForWindow(window_) / 96.0f)
        .SetFrame(0, 0, bounds.right, bounds.bottom).SetParent(window_);
    builder.RegisterNativeModule("LaneStore", lane::desktop::StoreModule, &store_handler_);
    builder.RegisterNativeModule("LaneHost", lane::desktop::HostModule, &host_handler_);
    view_ = builder.Build();
    if (!view_) throw std::runtime_error("Lynx view creation failed");
    view_->AddClient(client_);
    LoadLane(*view_, executable_.parent_path() / "main.lynx.bundle");
    ShowWindow(window_, SW_SHOW); SetForegroundWindow(window_);
    service_->Resume();
  }
  bool Activate(const std::string& uri) {
    Route route;
    if (!ParseRoute(uri, kScheme, route)) return false;
    if (route.kind == RouteKind::Callback) { service_->Callback(route.uri); Show(); return true; }
    Json value;
    Set(value, L"kind", route.kind == RouteKind::Task ? "task" : route.kind == RouteKind::Repository ? "repository" : "open");
    if (!route.id.empty()) Set(value, L"id", route.id);
    pending_route_ = value;
    Show(); SendRoute(); return true;
  }
  void SendRoute() {
    if (!pending_route_ || !view_ || !ready_) return;
    view_->SendGlobalEvent("lane.openRoute", "[" + Dump(*pending_route_) + "]");
    pending_route_.reset();
  }
  LRESULT Message(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
    if (window == controller_) {
      if (message == kDispatch) { ui_->Drain(); return 0; }
      if (message == WM_COPYDATA) {
        auto* data = reinterpret_cast<COPYDATASTRUCT*>(lparam);
        if (!data || data->dwData != kActivation || !data->lpData || data->cbData < sizeof(wchar_t) ||
            data->cbData > 32770 || data->cbData % sizeof(wchar_t)) return FALSE;
        const auto* text = static_cast<const wchar_t*>(data->lpData);
        const auto length = data->cbData / sizeof(wchar_t);
        if (text[length - 1] != 0 || wcsnlen(text, length) != length - 1) return FALSE;
        return Activate(winrt::to_string(text));
      }
      if (message == kTray) {
        if (LOWORD(lparam) == WM_LBUTTONUP || LOWORD(lparam) == NIN_SELECT) Show();
        if (LOWORD(lparam) == WM_RBUTTONUP || LOWORD(lparam) == WM_CONTEXTMENU) {
          HMENU menu = CreatePopupMenu();
          AppendMenuW(menu, MF_STRING, kOpen, L"Open Lane");
          AppendMenuW(menu, MF_STRING, kQuit, L"Quit Lane");
          POINT point{}; GetCursorPos(&point); SetForegroundWindow(controller_);
          const auto selected = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
              point.x, point.y, 0, controller_, nullptr);
          DestroyMenu(menu);
          if (selected == kOpen) Show();
          if (selected == kQuit) PostQuitMessage(0);
        }
        return 0;
      }
    }
    if (message == WM_COMMAND && LOWORD(wparam) == kQuit) { PostQuitMessage(0); return 0; }
    if (message == WM_QUERYENDSESSION) return TRUE;
    if (message == WM_ENDSESSION && wparam) { PostQuitMessage(0); return 0; }
    if (window == window_) {
      if (message == WM_SIZE && view_) view_->SetFrame(0, 0, LOWORD(lparam), HIWORD(lparam));
      if (message == WM_ACTIVATE && view_) {
        if (LOWORD(wparam) == WA_INACTIVE) view_->OnEnterBackground();
        else { view_->OnEnterForeground(); service_->Resume(); }
      }
      if (message == WM_POWERBROADCAST && wparam == PBT_APMRESUMEAUTOMATIC) service_->Resume();
      if (message == WM_DESTROY) {
        ready_ = false; ++generation_; view_.reset(); window_ = nullptr;
        return 0; // The controller and worker continue until explicit Quit.
      }
    }
    return DefWindowProcW(window, message, wparam, lparam);
  }
  void Shutdown() {
    if (stopped_) return;
    stopped_ = true;
    ui_->Stop(); // Wake waiting workers before joining the store worker.
    if (window_) DestroyWindow(window_);
    if (service_) service_->Stop();
    notifications_->Stop();
    if (tray_) { NOTIFYICONDATAW icon{}; icon.cbSize = sizeof(icon); icon.hWnd = controller_; icon.uID = 1; Shell_NotifyIconW(NIM_DELETE, &icon); }
    if (controller_) DestroyWindow(std::exchange(controller_, nullptr));
  }
 private:
  void AddTray() {
    NOTIFYICONDATAW icon{};
    icon.cbSize = sizeof(icon); icon.hWnd = controller_; icon.uID = 1;
    icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    icon.uCallbackMessage = kTray; icon.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
    wcscpy_s(icon.szTip, L"Lane");
    tray_ = Shell_NotifyIconW(NIM_ADD, &icon);
  }
  HINSTANCE instance_;
  std::filesystem::path executable_;
  std::wstring user_;
  HWND controller_ = nullptr, window_ = nullptr;
  bool stopped_ = false, ready_ = false, tray_ = false;
  uint64_t generation_ = 0;
  std::optional<Json> pending_route_;
  std::shared_ptr<UIQueue> ui_ = std::make_shared<UIQueue>();
  std::shared_ptr<Notifications> notifications_ = std::make_shared<Notifications>();
  std::shared_ptr<Service> service_;
  std::shared_ptr<lane::desktop::Handler> store_handler_, host_handler_;
  std::shared_ptr<LaneClient> client_ = std::make_shared<LaneClient>();
  std::unique_ptr<lynx::pub::LynxView> view_;
};
LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
  if (message == WM_NCCREATE) {
    auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
    SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(create->lpCreateParams));
  }
  auto* application = reinterpret_cast<Application*>(GetWindowLongPtrW(window, GWLP_USERDATA));
  try { return application ? application->Message(window, message, wparam, lparam) : DefWindowProcW(window, message, wparam, lparam); }
  catch (...) { std::cerr << "{\"event\":\"lane_windows_dispatch_failed\"}\n"; return 0; }
}
}  // namespace

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int) {
  try {
    winrt::init_apartment(winrt::apartment_type::single_threaded);
    struct Apartment { ~Apartment() { winrt::uninit_apartment(); } } apartment;
    wchar_t executable[32768];
    const auto length = GetModuleFileNameW(nullptr, executable, 32768);
    if (!length || length == 32768) return 1;
    int count = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &count);
    if (!argv) return 1;
    std::vector<std::wstring> arguments(argv, argv + count); LocalFree(argv);
    std::string uri = std::string(kScheme) + "://open";
    bool toast_activation = false;
    if (arguments.size() == 3 && arguments[1] == L"--uri") uri = winrt::to_string(arguments[2]);
    else if (arguments.size() == 2 && arguments[1] == L"-ToastActivated") toast_activation = true;
    else if (arguments.size() != 1) return 2;
    Route route;
    if (!ParseRoute(uri, kScheme, route)) return 2;
    const auto user = UserSID();
    const auto mutex_name = L"Local\\" + std::wstring(kControllerClass) + L"." + user;
    Handle instance_lock(CreateMutexW(nullptr, FALSE, mutex_name.c_str()));
    if (!instance_lock) return 1;
    if (GetLastError() == ERROR_ALREADY_EXISTS) return Forward(user, uri) ? 0 : 1;
    Application app(instance, executable, user);
    app.Start();
    if (!toast_activation) app.Activate(uri);
    std::clog << "{\"event\":\"lane_started\",\"platform\":\"windows\"}\n";
    MSG message{};
    int status;
    while ((status = GetMessageW(&message, nullptr, 0, 0)) > 0) {
      TranslateMessage(&message); DispatchMessageW(&message);
    }
    return status < 0 ? 1 : static_cast<int>(message.wParam);
  } catch (...) {
    std::cerr << "{\"event\":\"lane_start_failed\",\"platform\":\"windows\"}\n";
    return 1;
  }
}
