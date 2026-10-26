#include "capture.cc"
#include <iostream>

namespace {
using namespace lane::desktop;

void Check(bool ok, const char* message) {
  if (!ok) throw std::runtime_error(message);
}

void OnCaptureThread(WinCapture& host, std::function<void()> action) {
  std::promise<void> done;
  auto future = done.get_future();
  std::function<void()> work = [&] {
    try { action(); done.set_value(); }
    catch (...) { done.set_exception(std::current_exception()); }
  };
  Check(PostMessageW(host.window, kCall, 0, reinterpret_cast<LPARAM>(&work)) != FALSE,
        "Could not queue capture test");
  future.get();
}

void CheckPixel(const CaptureJSON& image, UINT pixelX, UINT pixelY,
                BYTE red, BYTE green, BYTE blue) {
  auto bytes = Decode(image.at("png"));
  Stream stream(SHCreateMemStream(bytes.data(), (UINT)bytes.size()),
                [](IStream* value) { value->Release(); });
  Check(stream != nullptr, "Could not open encoded PNG");
  std::unique_ptr<Gdiplus::Bitmap> bitmap(Gdiplus::Bitmap::FromStream(stream.get()));
  Check(bitmap && bitmap->GetLastStatus() == Gdiplus::Ok, "Encoded PNG is invalid");
  Gdiplus::Color pixel;
  Check(bitmap->GetPixel(pixelX, pixelY, &pixel) == Gdiplus::Ok, "Could not read PNG pixel");
  Check(pixel.GetR() == red && pixel.GetG() == green && pixel.GetB() == blue &&
        pixel.GetA() == 255, "PNG pixel or opacity mismatch");
}

void Run() {
  WinCapture host(std::make_shared<Events>());
  OnCaptureThread(host, [&] {
    Gdiplus::Bitmap source(20, 20, PixelFormat32bppARGB);
    Check(source.GetLastStatus() == Gdiplus::Ok, "Could not create fixture");
    for (INT pixelY = 0; pixelY < 20; ++pixelY)
      for (INT pixelX = 0; pixelX < 20; ++pixelX)
        Check(source.SetPixel(pixelX, pixelY, pixelY < 10 ? Gdiplus::Color::White : Gdiplus::Color::Black) == Gdiplus::Ok,
              "Could not write fixture pixel");
    auto png = PNG(source).at("png");
    auto render = [&](CaptureJSON layers, CaptureJSON crop = nullptr) {
      return host.Render({{"png", png}, {"layers", layers}, {"crop", crop}});
    };
    auto covered = render(CaptureJSON::array({{{"type", "cover"}, {"x", 0}, {"y", 0},
                            {"width", 5}, {"height", 5}, {"color", "#FF0000"}}}));
    Check(covered["width"] == 20 && covered["height"] == 20, "Render dimensions changed");
    Check(covered.at("png").get<std::string>().find('\0') == std::string::npos,
          "Base64 contains a terminator");
    CheckPixel(covered, 2, 2, 255, 0, 0);
    CheckPixel(covered, 10, 2, 255, 255, 255);
    CheckPixel(covered, 2, 18, 0, 0, 0);

    auto crop = render(CaptureJSON::array(), {{"x", 0}, {"y", 10}, {"width", 8}, {"height", 7}});
    Check(crop["width"] == 8 && crop["height"] == 7, "Crop dimensions mismatch");
    CheckPixel(crop, 2, 2, 0, 0, 0);
    auto flattened = render(CaptureJSON::array({{{"type", "cover"}, {"x", 0}, {"y", 0},
                              {"width", 5}, {"height", 5}, {"color", "#FF0000"}}}),
                            {{"x", 1}, {"y", 1}, {"width", 3}, {"height", 3}});
    CheckPixel(flattened, 1, 1, 255, 0, 0);

    for (auto type : {"draw", "arrow", "rectangle", "text", "blur", "cover"}) {
      auto image = render(CaptureJSON::array({{{"type", type}, {"x", 2}, {"y", 2},
                            {"width", 10}, {"height", 10}, {"text", "QA"}, {"color", "#0000FF"}}}));
      Check(image["width"] == 20, "Editor tool failed");
    }
    auto blurred = render(CaptureJSON::array({{{"type", "blur"}, {"x", 0}, {"y", 0},
                            {"width", 20}, {"height", 20}}}));
    auto blurredBytes = Decode(blurred.at("png"));
    Stream blurredStream(SHCreateMemStream(blurredBytes.data(), (UINT)blurredBytes.size()),
                         [](IStream* value) { value->Release(); });
    Check(blurredStream != nullptr, "Could not open blurred PNG");
    std::unique_ptr<Gdiplus::Bitmap> blurredBitmap(Gdiplus::Bitmap::FromStream(blurredStream.get()));
    Gdiplus::Color blurredPixel;
    Check(blurredBitmap && blurredBitmap->GetPixel(10, 9, &blurredPixel) == Gdiplus::Ok &&
          blurredPixel.GetR() > 0 && blurredPixel.GetR() < 255 && blurredPixel.GetA() == 255,
          "Blur must mix the boundary without losing opacity");
    auto outside = render(CaptureJSON::array({{{"type", "blur"}, {"x", 1e100}, {"y", 0},
                            {"width", 1e100}, {"height", 20}}}));
    CheckPixel(outside, 2, 2, 255, 255, 255);
    auto clipped = render(CaptureJSON::array({{{"type", "blur"}, {"x", -1e100}, {"y", 0},
                            {"width", 1e101}, {"height", 20}}}));
    Check(clipped["width"] == 20, "Blur clipping failed");
    auto expectInvalid = [&](const CaptureJSON& request) {
      bool rejected = false;
      try { host.Render(request); }
      catch (const CaptureError&) { rejected = true; }
      Check(rejected, "Invalid edit accepted");
    };
    expectInvalid({{"png", png}, {"layers", CaptureJSON::array()},
                   {"crop", {{"x", 19}, {"y", 0}, {"width", 8}, {"height", 7}}}});
    for (const auto& layer : CaptureJSON::array({
           {{"type", "unknown"}}, {{"type", "cover"}, {"width", -1}},
           {{"type", "cover"}, {"color", "#12GG34"}},
           {{"type", "text"}, {"fontSize", 0}}}))
      expectInvalid({{"png", png}, {"layers", CaptureJSON::array({layer})}});

    host.selecting = true;
    try {
      host.Capture({{"mode", "region"}});
      host.selecting = false;
      throw std::runtime_error("Reentrant capture accepted");
    } catch (const CaptureError& error) {
      host.selecting = false;
      Check(error.reason == "ERROR_REASON_CONFLICT", "Wrong reentrant capture error");
    }
  });

  auto call = [&](CaptureJSON request) { return CaptureJSON::parse(host.Invoke(request.dump())); };
  Check(!call({{"op", "render"}, {"png", "bad data"}, {"layers", CaptureJSON::array()}})["ok"],
        "Malformed PNG accepted");
  Check(!call({{"op", "render"}, {"png", "YWJj"}, {"layers", CaptureJSON::array()}})["ok"],
        "Non-image bytes accepted");
  Check(!call({{"op", "shortcut"}, {"mode", "region"}, {"accelerator", "Ctrl+Q"}})["ok"],
        "Reserved Lane shortcut accepted");
  Check(call({{"op", "permission"}})["ok"], "Capture queue failed after an error");
  OnCaptureThread(host, [&] {
    host.selecting = true;
    host.accepted = true;
    Check(PostMessageW(host.window, WM_CLOSE, 0, 0) != FALSE, "Could not queue shutdown");
    host.WaitForSelection();
    Check(!host.selecting && !host.accepted, "Host shutdown did not cancel selection");
  });
  host.thread.join();
  WinCapture quittingHost(std::make_shared<Events>());
  OnCaptureThread(quittingHost, [&] {
    quittingHost.selecting = true;
    quittingHost.dragging = true;
    quittingHost.accepted = true;
    PostQuitMessage(17);
    quittingHost.WaitForSelection();
    Check(!quittingHost.selecting && !quittingHost.dragging && !quittingHost.accepted,
          "Quit must reset selection state");
    MSG message{};
    Check(PeekMessageW(&message, nullptr, WM_QUIT, WM_QUIT, PM_NOREMOVE) && message.wParam == 17,
          "Selection must preserve WM_QUIT for the host loop");
  });
  quittingHost.thread.join();
}
}

int main() {
  try {
    Run();
    std::cout << "Lane Windows RealQA memory renderer tests passed\n";
    return 0;
  } catch (const std::exception& error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
