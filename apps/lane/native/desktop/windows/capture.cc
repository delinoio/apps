// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <windowsx.h>
#include <objidl.h>
#include <gdiplus.h>
#include <wincrypt.h>
#include <shlwapi.h>
#include <dwmapi.h>
#include <shellapi.h>
#include <thread>
#include <future>
#include <sstream>
#include <map>
#include <chrono>
#include <algorithm>
#include <limits>
#include <type_traits>
#include "../realqa.h"
#include "logging.h"

namespace lane::desktop {
namespace {
using namespace Gdiplus;
using Stream = std::unique_ptr<IStream, void(*)(IStream*)>;
std::wstring Wide(const std::string& text) {
  int size=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text.data(),(int)text.size(),nullptr,0);
  if (!size && !text.empty()) throw CaptureError("Invalid text.");
  std::wstring result(size,0); MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text.data(),(int)text.size(),result.data(),size); return result;
}
std::string UTF8(const std::wstring& text) {
  int size=WideCharToMultiByte(CP_UTF8,0,text.data(),(int)text.size(),nullptr,0,nullptr,nullptr);
  std::string result(size,0); WideCharToMultiByte(CP_UTF8,0,text.data(),(int)text.size(),result.data(),size,nullptr,nullptr); return result;
}
std::string Base64(const BYTE* bytes, DWORD size) {
  DWORD count=0;
  if (!CryptBinaryToStringA(bytes,size,CRYPT_STRING_BASE64|CRYPT_STRING_NOCRLF,nullptr,&count)) throw CaptureError("Image encoding failed.");
  std::string result(count,0); if (!CryptBinaryToStringA(bytes,size,CRYPT_STRING_BASE64|CRYPT_STRING_NOCRLF,result.data(),&count)) throw CaptureError("Image encoding failed.");
  result.resize(count); return result;
}
std::vector<BYTE> Decode(const std::string& value) {
  DWORD count=0;
  if (!CryptStringToBinaryA(value.data(),(DWORD)value.size(),CRYPT_STRING_BASE64,nullptr,&count,nullptr,nullptr)) throw CaptureError("Invalid image encoding.");
  std::vector<BYTE> result(count);
  if (!CryptStringToBinaryA(value.data(),(DWORD)value.size(),CRYPT_STRING_BASE64,result.data(),&count,nullptr,nullptr)) throw CaptureError("Invalid image encoding.");
  return result;
}
CaptureJSON PNG(Bitmap& image) {
  UINT count=0,size=0;
  if (GetImageEncodersSize(&count,&size)!=Ok || !size) throw CaptureError("PNG encoding is unavailable.");
  std::vector<BYTE> codecs(size);
  auto* encoders=reinterpret_cast<ImageCodecInfo*>(codecs.data());
  if (GetImageEncoders(count,size,encoders)!=Ok) throw CaptureError("PNG encoding is unavailable.");
  CLSID png{}; bool found=false;
  for (UINT i=0;i<count;i++) if (wcscmp(encoders[i].MimeType,L"image/png")==0) { png=encoders[i].Clsid; found=true; }
  IStream* stream=nullptr;
  if (!found || FAILED(CreateStreamOnHGlobal(nullptr,TRUE,&stream))) throw CaptureError("PNG encoding is unavailable.");
  Stream output(stream,[](IStream* value){value->Release();});
  if (image.Save(stream,&png,nullptr)!=Ok) throw CaptureError("Image encoding failed.");
  STATSTG stat{}; HGLOBAL memory=nullptr;
  if (FAILED(stream->Stat(&stat,STATFLAG_NONAME)) || stat.cbSize.HighPart || !stat.cbSize.LowPart || FAILED(GetHGlobalFromStream(stream,&memory))) throw CaptureError("Image encoding failed.");
  auto* bytes=static_cast<BYTE*>(GlobalLock(memory));
  if (!bytes) throw CaptureError("Image encoding failed.");
  std::string encoded;
  try { encoded=Base64(bytes,stat.cbSize.LowPart); } catch (...) { GlobalUnlock(memory); throw; }
  GlobalUnlock(memory);
  return {{"png",encoded},{"width",image.GetWidth()},{"height",image.GetHeight()}};
}
constexpr UINT kCall=WM_APP+71;
struct WinCapture {
  std::shared_ptr<Events> events;
  std::thread thread;
  HWND window=nullptr,overlay=nullptr;
  ULONG_PTR graphics=0;
  std::map<std::string,int> shortcuts;
  std::map<int,std::string> modes;
  int next=1;
  RECT selected{},monitor{}; POINT anchor{}; bool selecting=false, accepted=false,dragging=false;
  explicit WinCapture(std::shared_ptr<Events> sink):events(std::move(sink)) {
    std::promise<void> ready; auto future=ready.get_future();
    thread=std::thread([this,&ready] {
      CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
      // Match monitor/window physical coordinates even when the application has
      // a different default DPI context. No process-wide DPI policy is changed.
      SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
      GdiplusStartupInput input;
      if (GdiplusStartup(&graphics,&input,nullptr)!=Ok) { ready.set_exception(std::make_exception_ptr(CaptureError("Image rendering is unavailable."))); CoUninitialize(); return; }
      WNDCLASSW cls{}; cls.lpfnWndProc=Dispatch; cls.hInstance=GetModuleHandleW(nullptr); cls.lpszClassName=L"LaneRealQACapture"; cls.hCursor=LoadCursor(nullptr,IDC_CROSS);
      RegisterClassW(&cls);
      window=CreateWindowExW(0,cls.lpszClassName,L"",0,0,0,0,0,HWND_MESSAGE,nullptr,cls.hInstance,this);
      if (!window) { ready.set_exception(std::make_exception_ptr(CaptureError("Capture host could not start."))); GdiplusShutdown(graphics); CoUninitialize(); return; }
      ready.set_value(); MSG msg;
      while (GetMessageW(&msg,nullptr,0,0)>0) { TranslateMessage(&msg); DispatchMessageW(&msg); }
      for (auto& pair:shortcuts) UnregisterHotKey(window,pair.second);
      DestroyWindow(window); GdiplusShutdown(graphics); CoUninitialize();
    });
    try { future.get(); } catch (...) { thread.join(); throw; }
  }
  ~WinCapture() { if (thread.joinable()) { PostMessageW(window,WM_CLOSE,0,0);thread.join(); } }
  static LRESULT CALLBACK Dispatch(HWND hwnd,UINT message,WPARAM wp,LPARAM lp) {
    if (message==WM_NCCREATE) SetWindowLongPtrW(hwnd,GWLP_USERDATA,reinterpret_cast<LONG_PTR>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams));
    auto* self=reinterpret_cast<WinCapture*>(GetWindowLongPtrW(hwnd,GWLP_USERDATA));
    if (!self) return DefWindowProcW(hwnd,message,wp,lp);
    if (message==kCall) { (*reinterpret_cast<std::function<void()>*>(lp))(); return 0; }
    if (message==WM_HOTKEY) {
      auto mode=self->modes.find((int)wp);
      if (mode!=self->modes.end()) self->events->Push("lane.realqa.shortcut",CaptureJSON::array({{{"mode",mode->second}}}).dump());
      return 0;
    }
    if (message==WM_CLOSE) { self->accepted=false;self->selecting=false;if(hwnd!=self->overlay)PostQuitMessage(0);return 0; }
    if (hwnd==self->overlay) {
      if (message==WM_KEYDOWN) {
        if (wp==VK_ESCAPE) self->selecting=false;
        else if (wp==VK_RETURN) { self->accepted=true; self->selecting=false; }
        else {
          RECT r=self->selected;
          int dx=wp==VK_LEFT?-1:wp==VK_RIGHT?1:0,dy=wp==VK_UP?-1:wp==VK_DOWN?1:0;
          if (GetKeyState(VK_SHIFT)<0) { r.right+=dx;r.bottom+=dy; } else OffsetRect(&r,dx,dy);
          if (r.left>=0 && r.top>=0 && r.right<=self->monitor.right-self->monitor.left && r.bottom<=self->monitor.bottom-self->monitor.top && r.right>r.left && r.bottom>r.top) self->selected=r;
          auto label=L"RealQA region: "+std::to_wstring(self->selected.left)+L", "+std::to_wstring(self->selected.top)+L", "+std::to_wstring(self->selected.right-self->selected.left)+L" by "+std::to_wstring(self->selected.bottom-self->selected.top)+L". Arrows move; Shift arrows resize; Enter captures; Escape cancels.";
          SetWindowTextW(hwnd,label.c_str());NotifyWinEvent(EVENT_OBJECT_NAMECHANGE,hwnd,OBJID_WINDOW,CHILDID_SELF);
          InvalidateRect(hwnd,nullptr,TRUE);
        }
        return 0;
      }
      if (message==WM_LBUTTONDOWN) { self->anchor={GET_X_LPARAM(lp),GET_Y_LPARAM(lp)};self->dragging=true;SetCapture(hwnd);return 0; }
      if (message==WM_MOUSEMOVE && self->dragging) {
        int x=std::clamp((int)GET_X_LPARAM(lp),0,(int)(self->monitor.right-self->monitor.left));
        int y=std::clamp((int)GET_Y_LPARAM(lp),0,(int)(self->monitor.bottom-self->monitor.top));
        self->selected={std::min((LONG)x,self->anchor.x),std::min((LONG)y,self->anchor.y),std::max((LONG)x,self->anchor.x),std::max((LONG)y,self->anchor.y)};
        InvalidateRect(hwnd,nullptr,TRUE);return 0;
      }
      if (message==WM_LBUTTONUP && self->dragging) { ReleaseCapture();self->dragging=false;self->accepted=self->selected.right>self->selected.left&&self->selected.bottom>self->selected.top;self->selecting=!self->accepted;return 0; }
      if (message==WM_PAINT) {
        PAINTSTRUCT paint{};HDC dc=BeginPaint(hwnd,&paint);RECT client{};GetClientRect(hwnd,&client);FillRect(dc,&client,(HBRUSH)GetStockObject(BLACK_BRUSH));
        HBRUSH brush=CreateSolidBrush(RGB(70,140,255));FrameRect(dc,&self->selected,brush);DeleteObject(brush);
        SetTextColor(dc,RGB(255,255,255));SetBkMode(dc,TRANSPARENT);
        const wchar_t* label=L"Drag a region. Arrows move; Shift + arrows resize. Enter captures. Esc cancels.";
        TextOutW(dc,24,24,label,(int)wcslen(label));EndPaint(hwnd,&paint);return 0;
      }
    }
    return DefWindowProcW(hwnd,message,wp,lp);
  }
  void WaitForSelection() {
    MSG message{};int status=1;
    while (selecting && (status=GetMessageW(&message,nullptr,0,0))>0) { TranslateMessage(&message);DispatchMessageW(&message); }
    selecting=false;dragging=false;
    if(status<=0) {accepted=false;if(status==0)PostQuitMessage((int)message.wParam);}
  }
  CaptureJSON Region() {
    POINT point{};
    if(!GetCursorPos(&point))throw CaptureError("The current monitor is unavailable.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true);
    HMONITOR display=MonitorFromPoint(point,MONITOR_DEFAULTTONEAREST);
    MONITORINFO info{sizeof(info)};
    if(!GetMonitorInfoW(display,&info))throw CaptureError("The current monitor is unavailable.","ERROR_REASON_NOT_FOUND",true);
    monitor=info.rcMonitor;
    selected={30,70,std::min(430L,monitor.right-monitor.left-30),std::min(370L,monitor.bottom-monitor.top-30)};
    selecting=true;accepted=false;dragging=false;
    overlay=CreateWindowExW(WS_EX_TOPMOST|WS_EX_LAYERED,L"LaneRealQACapture",L"RealQA region: arrows move, Shift arrows resize, Enter captures, Escape cancels",WS_POPUP,
      monitor.left,monitor.top,monitor.right-monitor.left,monitor.bottom-monitor.top,nullptr,nullptr,GetModuleHandleW(nullptr),this);
    if (!overlay) { selecting=false;throw CaptureError("Region selection could not open.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true); }
    SetLayeredWindowAttributes(overlay,0,110,LWA_ALPHA);ShowWindow(overlay,SW_SHOW);SetForegroundWindow(overlay);SetFocus(overlay);
    WaitForSelection();
    DestroyWindow(overlay);overlay=nullptr;DwmFlush();
    if (!accepted) return {{"cancelled",true}};
    return {{"sourceId",std::to_string(reinterpret_cast<uintptr_t>(display))},{"rect",{{"x",selected.left},{"y",selected.top},{"width",selected.right-selected.left},{"height",selected.bottom-selected.top}}}};
  }
  CaptureJSON Sources(const CaptureJSON& request) {
    auto sources=CaptureJSON::array();
    if (request.value("mode","")=="screen") EnumDisplayMonitors(nullptr,nullptr,[](HMONITOR monitor,HDC,LPRECT,LPARAM ptr)->BOOL {
      auto& out=*reinterpret_cast<CaptureJSON*>(ptr);MONITORINFOEXW info{};info.cbSize=sizeof(info);GetMonitorInfoW(monitor,&info);
      out.push_back({{"id",std::to_string(reinterpret_cast<uintptr_t>(monitor))},{"name",UTF8(info.szDevice)},{"width",info.rcMonitor.right-info.rcMonitor.left},{"height",info.rcMonitor.bottom-info.rcMonitor.top}});return TRUE;
    },reinterpret_cast<LPARAM>(&sources));
    else EnumWindows([](HWND window,LPARAM ptr)->BOOL {
      DWORD process=0;GetWindowThreadProcessId(window,&process);DWORD cloaked=0;DwmGetWindowAttribute(window,DWMWA_CLOAKED,&cloaked,sizeof(cloaked));
      if (!IsWindowVisible(window)||IsIconic(window)||cloaked||process==GetCurrentProcessId()) return TRUE;
      int count=GetWindowTextLengthW(window);if (!count) return TRUE;std::wstring title(count+1,0);GetWindowTextW(window,title.data(),count+1);title.resize(count);
      RECT rect{};GetWindowRect(window,&rect);if(rect.right<=rect.left||rect.bottom<=rect.top)return TRUE;
      auto& out=*reinterpret_cast<CaptureJSON*>(ptr);out.push_back({{"id",std::to_string(reinterpret_cast<uintptr_t>(window))},{"name",UTF8(title)},{"width",rect.right-rect.left},{"height",rect.bottom-rect.top}});return TRUE;
    },reinterpret_cast<LPARAM>(&sources));
    return {{"sources",sources}};
  }
  CaptureJSON Capture(CaptureJSON request) {
    if(selecting)throw CaptureError("Finish or cancel the current region selection first.","ERROR_REASON_CONFLICT",true);
    auto mode=request.value("mode","");
    if (mode=="region" && !request.contains("rect")) {auto region=Region();if(region.value("cancelled",false))return region;request.update(region);}
    std::string id=request.value("sourceId","");
    auto sources=Sources({{"mode",mode=="window"?"window":"screen"}})["sources"];
    if (std::none_of(sources.begin(),sources.end(),[&](const auto& source){return source["id"]==id;})) throw CaptureError("The selected source is unavailable. Select it again.","ERROR_REASON_NOT_FOUND",true);
    auto handle=static_cast<uintptr_t>(std::stoull(id));RECT rect{};HWND target=nullptr;
    if(mode=="window") {target=reinterpret_cast<HWND>(handle);if(!GetWindowRect(target,&rect))throw CaptureError("The selected window is unavailable. Select it again.","ERROR_REASON_NOT_FOUND",true);} else {MONITORINFO info{sizeof(info)};if(!GetMonitorInfoW(reinterpret_cast<HMONITOR>(handle),&info))throw CaptureError("The selected monitor is unavailable. Select it again.","ERROR_REASON_NOT_FOUND",true);rect=info.rcMonitor;}
    if(request.contains("rect")) {
      auto r=request["rect"];double x=Coordinate(r,"x"),y=Coordinate(r,"y"),w=Coordinate(r,"width"),h=Coordinate(r,"height");
      if(x<0||y<0||w<1||h<1||x+w>rect.right-rect.left||y+h>rect.bottom-rect.top)throw CaptureError("Select a region inside one monitor.");
      rect={rect.left+(LONG)x,rect.top+(LONG)y,rect.left+(LONG)(x+w),rect.top+(LONG)(y+h)};
    }
    int width=rect.right-rect.left,height=rect.bottom-rect.top;
    if(width<=0||height<=0)throw CaptureError("The selected source is unavailable. Select it again.","ERROR_REASON_NOT_FOUND",true);
    HDC screen=GetDC(nullptr),memory=CreateCompatibleDC(screen);HBITMAP bitmap=CreateCompatibleBitmap(screen,width,height);
    if(!screen||!memory||!bitmap) {if(bitmap)DeleteObject(bitmap);if(memory)DeleteDC(memory);if(screen)ReleaseDC(nullptr,screen);throw CaptureError("Screen capture is unavailable.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true);}
    auto previous=SelectObject(memory,bitmap);
    BOOL success=target?PrintWindow(target,memory,2):BitBlt(memory,0,0,width,height,screen,rect.left,rect.top,SRCCOPY|CAPTUREBLT);
    SelectObject(memory,previous);DeleteDC(memory);ReleaseDC(nullptr,screen);
    if(!success){DeleteObject(bitmap);throw CaptureError("Screen capture failed. Check Windows privacy settings or protected-window restrictions and retry.","ERROR_REASON_PERMISSION_DENIED",true);}
    std::unique_ptr<std::remove_pointer_t<HBITMAP>,decltype(&DeleteObject)> captured(bitmap,&DeleteObject);
    Bitmap image(bitmap,nullptr);return PNG(image);
  }
  CaptureJSON Render(const CaptureJSON& request) {
    auto bytes=Decode(request.at("png"));
    Stream input(SHCreateMemStream(bytes.data(),(UINT)bytes.size()),[](IStream* value){value->Release();});
    if(!input)throw CaptureError("Image decoding is unavailable.");
    std::unique_ptr<Bitmap> original(Bitmap::FromStream(input.get()));
    if(!original||original->GetLastStatus()!=Ok)throw CaptureError("The captured image is invalid.");
    UINT width=original->GetWidth(),height=original->GetHeight();
    ValidateEdits(request,width,height);
    if(!width||!height||width>(UINT)std::numeric_limits<INT>::max()||height>(UINT)std::numeric_limits<INT>::max())throw CaptureError("The captured image is too large.");
    Bitmap canvas(width,height,PixelFormat32bppARGB);
    if(canvas.GetLastStatus()!=Ok)throw CaptureError("Image rendering is unavailable.");
    { Graphics graphics(&canvas);if(graphics.DrawImage(original.get(),0,0,(INT)width,(INT)height)!=Ok)throw CaptureError("Image rendering failed."); }
    original.reset();input.reset();
    for(const auto& layer:request["layers"]) {
      auto type=layer.value("type","");REAL x=(REAL)Coordinate(layer,"x"),y=(REAL)Coordinate(layer,"y"),w=(REAL)Coordinate(layer,"width"),h=(REAL)Coordinate(layer,"height");
      unsigned long rgb=std::stoul(layer.value("color","#2563EB").substr(1),nullptr,16);Color color(255,(BYTE)(rgb>>16),(BYTE)(rgb>>8),(BYTE)rgb);
      if(type=="blur"&&w>0&&h>0) {
        Rect area((INT)std::clamp(std::floor(Coordinate(layer,"x")),0.0,(double)width),(INT)std::clamp(std::floor(Coordinate(layer,"y")),0.0,(double)height),0,0);
        area.Width=(INT)std::clamp(std::ceil(Coordinate(layer,"x")+Coordinate(layer,"width")),0.0,(double)width)-area.X;
        area.Height=(INT)std::clamp(std::ceil(Coordinate(layer,"y")+Coordinate(layer,"height")),0.0,(double)height)-area.Y;
        if(area.Width<=0||area.Height<=0)continue;
        std::vector<BYTE> horizontal((size_t)area.Width*area.Height*4);
        BitmapData data{};if(canvas.LockBits(&area,ImageLockModeRead|ImageLockModeWrite,PixelFormat32bppARGB,&data)!=Ok)throw CaptureError("Blur could not render.");
        // A separable box blur reads only the selected pixels and writes the
        // flattened result. No source pixels or layers are persisted.
        for(int yy=0;yy<area.Height;yy++) for(int c=0;c<4;c++) {
          auto* row=(BYTE*)data.Scan0+yy*data.Stride;int sum=0;
          for(int xx=-12;xx<=12;xx++)sum+=row[std::clamp(xx,0,area.Width-1)*4+c];
          for(int xx=0;xx<area.Width;xx++){horizontal[((size_t)yy*area.Width+xx)*4+c]=(BYTE)(sum/25);sum-=row[std::clamp(xx-12,0,area.Width-1)*4+c];sum+=row[std::clamp(xx+13,0,area.Width-1)*4+c];}
        }
        for(int xx=0;xx<area.Width;xx++)for(int c=0;c<4;c++) {
          int sum=0;for(int yy=-12;yy<=12;yy++)sum+=horizontal[((size_t)std::clamp(yy,0,area.Height-1)*area.Width+xx)*4+c];
          for(int yy=0;yy<area.Height;yy++){*((BYTE*)data.Scan0+yy*data.Stride+xx*4+c)=(BYTE)(sum/25);sum-=horizontal[((size_t)std::clamp(yy-12,0,area.Height-1)*area.Width+xx)*4+c];sum+=horizontal[((size_t)std::clamp(yy+13,0,area.Height-1)*area.Width+xx)*4+c];}
        }
        if(canvas.UnlockBits(&data)!=Ok)throw CaptureError("Blur could not render.");continue;
      }
      Graphics graphics(&canvas);graphics.SetSmoothingMode(SmoothingModeAntiAlias);Pen pen(color,(REAL)Coordinate(layer,"strokeWidth",3));SolidBrush brush(color);
      if(type=="cover") {if(graphics.FillRectangle(&brush,x,y,w,h)!=Ok)throw CaptureError("Cover could not render.");}
      else if(type=="rectangle") {if(graphics.DrawRectangle(&pen,x,y,w,h)!=Ok)throw CaptureError("Rectangle could not render.");}
      else if(type=="text") {Font font(L"Segoe UI",(REAL)Coordinate(layer,"fontSize",20),FontStyleRegular,UnitPixel);auto text=Wide(layer.value("text",""));if(graphics.DrawString(text.c_str(),(INT)text.size(),&font,PointF(x,y),&brush)!=Ok)throw CaptureError("Text could not render.");}
      else if(type=="draw"||type=="arrow") {
        std::vector<PointF> points;
        if(layer.contains("points"))for(const auto& p:layer["points"])points.emplace_back((REAL)Coordinate(p,"x"),(REAL)Coordinate(p,"y"));
        if(points.empty())points={PointF(x,y),PointF(x+w,y+h)};
        if(points.size()>=2)graphics.DrawLines(&pen,points.data(),(INT)points.size());
        if(type=="arrow"&&points.size()>=2) {auto a=points[points.size()-2],b=points.back();REAL angle=atan2f(b.Y-a.Y,b.X-a.X),length=std::max(12.0f,pen.GetWidth()*4);graphics.DrawLine(&pen,b,PointF(b.X-length*cosf(angle-0.5f),b.Y-length*sinf(angle-0.5f)));graphics.DrawLine(&pen,b,PointF(b.X-length*cosf(angle+0.5f),b.Y-length*sinf(angle+0.5f)));}
      }
    }
    if(request.contains("crop")&&!request["crop"].is_null()) {auto c=request["crop"];std::unique_ptr<Bitmap> cropped(canvas.Clone((INT)Coordinate(c,"x"),(INT)Coordinate(c,"y"),(INT)Coordinate(c,"width"),(INT)Coordinate(c,"height"),PixelFormat32bppARGB));if(!cropped||cropped->GetLastStatus()!=Ok)throw CaptureError("Crop failed.");return PNG(*cropped);}
    return PNG(canvas);
  }
  CaptureJSON Shortcut(const CaptureJSON& request) {
    std::string mode=request.at("mode"),value=request.at("accelerator");auto old=shortcuts.find(mode);
    if(value.empty()){if(old!=shortcuts.end()){UnregisterHotKey(window,old->second);modes.erase(old->second);shortcuts.erase(old);}return CaptureJSON::object();}
    UINT modifiers=MOD_NOREPEAT,key=0;std::istringstream stream(value);std::string token;
    while(std::getline(stream,token,'+')) {if(token=="Ctrl"||token=="Control")modifiers|=MOD_CONTROL;else if(token=="Alt"||token=="Option")modifiers|=MOD_ALT;else if(token=="Shift")modifiers|=MOD_SHIFT;else if(token=="Meta"||token=="Win"||token=="Command")modifiers|=MOD_WIN;else if(token.size()==1&&((token[0]>='A'&&token[0]<='Z')||(token[0]>='0'&&token[0]<='9')))key=token[0];else throw CaptureError("Use modifiers and a letter or digit for the shortcut.");}
    if(modifiers==(MOD_NOREPEAT|MOD_CONTROL)&&(key=='Q'||key=='O'||key=='K'))throw CaptureError("Lane already uses that shortcut.","ERROR_REASON_CONFLICT");
    int id=next++;
    if(!key||modifiers==MOD_NOREPEAT||!RegisterHotKey(window,id,modifiers,key))throw CaptureError("That shortcut is unavailable or already used. The previous shortcut is retained.","ERROR_REASON_CONFLICT");
    if(old!=shortcuts.end()){UnregisterHotKey(window,old->second);modes.erase(old->second);}shortcuts[mode]=id;modes[id]=mode;return CaptureJSON::object();
  }
  CaptureJSON Call(const CaptureJSON& request) {
    auto op=request.value("op","");
    if(op=="cancel"){if(overlay)PostMessageW(overlay,WM_CLOSE,0,0);return CaptureJSON::object();}
    if(op=="sources")return Sources(request);if(op=="capture")return Capture(request);if(op=="render")return Render(request);if(op=="shortcut")return Shortcut(request);
    if(op=="permission")return {{"permission","available"},{"message","Protected windows and Windows privacy policy can prevent capture."}};
    if(op=="openPermissionSettings"){ShellExecuteW(nullptr,L"open",L"ms-settings:privacy-screenshot",nullptr,nullptr,SW_SHOWNORMAL);return CaptureJSON::object();}
    if(op=="activeBrowser") {DWORD pid=0;GetWindowThreadProcessId(GetForegroundWindow(),&pid);HANDLE process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,pid);wchar_t path[32768];DWORD size=32768;bool chrome=process&&QueryFullProcessImageNameW(process,0,path,&size)&&_wcsicmp(PathFindFileNameW(path),L"chrome.exe")==0;if(process)CloseHandle(process);return {{"chrome",chrome}};}
    throw CaptureError("Unknown capture operation.");
  }
  std::string Invoke(const std::string& raw) {
    std::promise<std::string> done;auto future=done.get_future();
    std::function<void()> work=[&] {
      const auto started=std::chrono::steady_clock::now();
      std::string operation="unknown";
      const auto log=[&](const char* event) { lane::windows::Log(event,operation.c_str(),0,std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now()-started).count()); };
      try {auto request=CaptureJSON::parse(raw);auto candidate=request.value("op","");if(candidate=="capture"||candidate=="render"||candidate=="sources"||candidate=="shortcut"||candidate=="permission"||candidate=="cancel")operation=candidate;done.set_value(CaptureJSON({{"ok",true},{"value",Call(request)}}).dump());log("lane_realqa_operation_completed");}
      catch(const CaptureError& error){done.set_value(CaptureFailure(error).dump());log("lane_realqa_operation_failed");}
      catch(...){done.set_value(CaptureFailure(CaptureError("Capture failed. Your draft is retained.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true)).dump());}
    };
    if(!PostMessageW(window,kCall,0,reinterpret_cast<LPARAM>(&work)))throw CaptureError("Capture host is unavailable.");return future.get();
  }
};
}
std::shared_ptr<Handler> CreateCaptureHost(std::shared_ptr<Events> events) {
  auto capture=std::make_shared<WinCapture>(std::move(events));
  return std::make_shared<Handler>([capture](const auto&,const auto& args){return capture->Invoke(args.at(0));});
}
}
