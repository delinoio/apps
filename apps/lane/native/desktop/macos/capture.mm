// SPDX-License-Identifier: Apache-2.0
#import <Cocoa/Cocoa.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreImage/CoreImage.h>
#import <Carbon/Carbon.h>
#import <ImageIO/ImageIO.h>
#import <objc/runtime.h>
#include <sstream>
#include <dlfcn.h>
#include "../realqa.h"
#include "log.h"
#include <map>
#include <chrono>
#include <cmath>

using lane::desktop::CaptureJSON;
using lane::desktop::CaptureError;
namespace {
NSString* Text(const std::string& s) { return [[NSString alloc] initWithBytes:s.data() length:s.size() encoding:NSUTF8StringEncoding]; }
void Main(dispatch_block_t block) {
  if (NSThread.isMainThread) block(); else dispatch_sync(dispatch_get_main_queue(), block);
}
CaptureJSON PNG(CGImageRef image) {
  if (!image) throw CaptureError("Capture failed. Retry the capture.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE", true);
  NSMutableData* data = [NSMutableData data];
  CGImageDestinationRef output = CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data, CFSTR("public.png"), 1, nullptr);
  if (!output) throw CaptureError("The PNG encoder is unavailable.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE", true);
  CGImageDestinationAddImage(output, image, nullptr);
  bool success = CGImageDestinationFinalize(output); CFRelease(output);
  if (!success) throw CaptureError("The image could not be encoded.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE", true);
  return {{"png", [data base64EncodedStringWithOptions:0].UTF8String}, {"width", CGImageGetWidth(image)}, {"height", CGImageGetHeight(image)}};
}
SCShareableContent* Content() API_AVAILABLE(macos(12.3));
SCShareableContent* Content() {
  if (!CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess())
    throw CaptureError("Allow Lane in System Settings > Privacy & Security > Screen & System Audio Recording, then retry.", "ERROR_REASON_PERMISSION_DENIED", true);
  auto done = dispatch_semaphore_create(0);
  __block SCShareableContent* content = nil;
  [SCShareableContent getShareableContentExcludingDesktopWindows:YES onScreenWindowsOnly:YES completionHandler:^(SCShareableContent* value, NSError*) {
    content = value; dispatch_semaphore_signal(done);
  }];
  dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
  if (!content) throw CaptureError("Screen sources are unavailable. Check screen-recording permission and retry.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE", true);
  return content;
}
}

@interface LaneRegionView : NSView
@property NSRect selection;
@property NSPoint anchor;
@property(copy) void (^finish)(BOOL);
@end
@implementation LaneRegionView
- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstResponder { return YES; }
- (void)drawRect:(NSRect)dirty {
  [[NSColor colorWithWhite:0 alpha:0.22] setFill]; NSRectFill(self.bounds);
  [[NSColor systemBlueColor] setStroke]; NSFrameRectWithWidth(self.selection, 3);
  NSString* label = @"Drag to select. Arrow keys move; Shift + arrows resize. Return captures. Esc cancels.";
  [label drawAtPoint:NSMakePoint(24, 24) withAttributes:@{NSFontAttributeName:[NSFont systemFontOfSize:17], NSForegroundColorAttributeName:NSColor.whiteColor, NSBackgroundColorAttributeName:NSColor.blackColor}];
}
- (void)mouseDown:(NSEvent*)event { self.anchor = [self convertPoint:event.locationInWindow fromView:nil]; }
- (void)mouseDragged:(NSEvent*)event {
  NSPoint point = [self convertPoint:event.locationInWindow fromView:nil];
  self.selection = NSIntersectionRect(self.bounds, NSMakeRect(MIN(point.x,self.anchor.x), MIN(point.y,self.anchor.y), fabs(point.x-self.anchor.x), fabs(point.y-self.anchor.y)));
  self.needsDisplay = YES;
}
- (void)mouseUp:(NSEvent*)event { if (self.selection.size.width >= 1 && self.selection.size.height >= 1) self.finish(YES); }
- (void)keyDown:(NSEvent*)event {
  if (event.keyCode == 53) { self.finish(NO); return; }
  if (event.keyCode == 36) { self.finish(YES); return; }
  NSRect r = self.selection;
  double dx = event.keyCode == 123 ? -1 : event.keyCode == 124 ? 1 : 0;
  double dy = event.keyCode == 126 ? -1 : event.keyCode == 125 ? 1 : 0;
  if (event.modifierFlags & NSEventModifierFlagShift) { r.size.width += dx; r.size.height += dy; }
  else { r.origin.x += dx; r.origin.y += dy; }
  if (r.size.width >= 1 && r.size.height >= 1 && NSContainsRect(self.bounds, r)) self.selection = r;
  self.needsDisplay = YES;
  [self setAccessibilityValue:[NSString stringWithFormat:@"Region %.0f, %.0f, %.0f by %.0f", self.selection.origin.x,self.selection.origin.y,self.selection.size.width,self.selection.size.height]];
  NSAccessibilityPostNotification(self,NSAccessibilityValueChangedNotification);
}
@end
@interface LaneRegionWindow : NSWindow
@end
@implementation LaneRegionWindow
- (BOOL)canBecomeKeyWindow { return YES; }
@end

namespace lane::desktop {
namespace {
struct MacCapture : std::enable_shared_from_this<MacCapture> {
  std::shared_ptr<Events> events;
  std::map<std::string, EventHotKeyRef> shortcuts;
  std::map<UInt32, std::string> modes;
  EventHandlerRef handler = nullptr;
  UInt32 next = 1;
  __weak LaneRegionView* selectionView = nil;
  explicit MacCapture(std::shared_ptr<Events> sink) : events(std::move(sink)) {
    EventTypeSpec type{kEventClassKeyboard, kEventHotKeyPressed};
    InstallApplicationEventHandler([](EventHandlerCallRef, EventRef event, void* ptr) -> OSStatus {
      auto& self = *static_cast<MacCapture*>(ptr);
      EventHotKeyID key{};
      GetEventParameter(event, kEventParamDirectObject, typeEventHotKeyID, nullptr, sizeof(key), nullptr, &key);
      auto found = self.modes.find(key.id);
      if (found != self.modes.end()) self.events->Push("lane.realqa.shortcut", CaptureJSON::array({{{"mode",found->second}}}).dump());
      return noErr;
    }, 1, &type, this, &handler);
  }
  ~MacCapture() {
    for (auto& item : shortcuts) UnregisterEventHotKey(item.second);
    if (handler) RemoveEventHandler(handler);
  }
  CaptureJSON LegacySources(const CaptureJSON& request) {
    if (!CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess())
      throw CaptureError("Allow Lane in System Settings > Privacy & Security > Screen Recording, then retry.","ERROR_REASON_PERMISSION_DENIED",true);
    auto sources=CaptureJSON::array();
    if(request.value("mode","")=="screen") {
      uint32_t count=0;CGGetActiveDisplayList(0,nullptr,&count);std::vector<CGDirectDisplayID> displays(count);CGGetActiveDisplayList(count,displays.data(),&count);
      for(auto display:displays)sources.push_back({{"id",std::to_string(display)},{"name","Monitor "+std::to_string(sources.size()+1)},{"width",CGDisplayPixelsWide(display)},{"height",CGDisplayPixelsHigh(display)}});
    } else {
      CFArrayRef windows=CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
      for(NSDictionary* info in (__bridge NSArray*)windows) {
        if([info[(__bridge NSString*)kCGWindowOwnerPID] intValue]==getpid() || [info[(__bridge NSString*)kCGWindowLayer] intValue]!=0)continue;
        CGRect bounds{};CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)info[(__bridge NSString*)kCGWindowBounds],&bounds);
        if(bounds.size.width<1||bounds.size.height<1)continue;
        NSString* name=info[(__bridge NSString*)kCGWindowName] ?: info[(__bridge NSString*)kCGWindowOwnerName];
        sources.push_back({{"id",std::to_string([info[(__bridge NSString*)kCGWindowNumber] unsignedIntValue])},{"name",name.UTF8String ?: "Window"},{"width",bounds.size.width},{"height",bounds.size.height}});
      }
      if(windows)CFRelease(windows);
    }
    return {{"sources",sources}};
  }
  CaptureJSON Sources(const CaptureJSON& request) {
    if (@available(macOS 14.0,*)) {
    auto content = Content(); auto sources = CaptureJSON::array();
    if (request.value("mode", "") == "screen") {
      for (SCDisplay* display in content.displays) sources.push_back({{"id",std::to_string(display.displayID)}, {"name","Monitor " + std::to_string(sources.size()+1)}, {"width",display.width}, {"height",display.height}});
    } else {
      for (SCWindow* window in content.windows) if (window.owningApplication.processID != NSProcessInfo.processInfo.processIdentifier && window.frame.size.width > 0 && window.frame.size.height > 0) {
        NSString* name = window.title.length ? window.title : window.owningApplication.applicationName;
        sources.push_back({{"id",std::to_string(window.windowID)}, {"name",name.UTF8String ?: "Window"}, {"width",window.frame.size.width}, {"height",window.frame.size.height}});
      }
    }
    return {{"sources",sources}};
    } else return LegacySources(request);
  }
  CaptureJSON SelectRegion() {
    auto done = dispatch_semaphore_create(0);
    __block CaptureJSON result = {{"cancelled",true}};
    Main(^{
      NSScreen* target = NSScreen.mainScreen;
      for (NSScreen* screen in NSScreen.screens) if (NSPointInRect(NSEvent.mouseLocation,screen.frame)) target = screen;
      LaneRegionWindow* overlay = [[LaneRegionWindow alloc] initWithContentRect:target.frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
      overlay.level = NSScreenSaverWindowLevel; overlay.opaque = NO;
      overlay.backgroundColor = NSColor.clearColor; overlay.releasedWhenClosed = NO;
      overlay.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
      LaneRegionView* view = [[LaneRegionView alloc] initWithFrame:NSMakeRect(0,0,target.frame.size.width,target.frame.size.height)];
      selectionView = view;
      view.selection = NSMakeRect(30,70,MIN(400,target.frame.size.width-60),MIN(300,target.frame.size.height-100));
      [view setAccessibilityElement:YES];
      [view setAccessibilityRole:NSAccessibilityGroupRole];
      [view setAccessibilityLabel:@"Capture region. Use arrows to move, Shift and arrows to resize, Return to capture, Escape to cancel."];
      __weak LaneRegionWindow* weakWindow = overlay;
      __weak LaneRegionView* weakView = view;
      NSNumber* displayID = target.deviceDescription[@"NSScreenNumber"];
      view.finish = ^(BOOL accepted) {
        if (accepted) {
          NSRect r = weakView.selection;
          result = {{"sourceId",std::to_string(displayID.unsignedIntValue)}, {"rect",{{"x",r.origin.x},{"y",r.origin.y},{"width",r.size.width},{"height",r.size.height}}}};
        }
        selectionView = nil;
        [weakWindow orderOut:nil]; [weakWindow close];
        objc_setAssociatedObject(weakView, @selector(finish), nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        dispatch_semaphore_signal(done);
      };
      overlay.contentView = view; [overlay makeKeyAndOrderFront:nil]; [overlay makeFirstResponder:view];
      // The window retains its content until selection completes; keep the
      // window alive without writing a preview to disk.
      objc_setAssociatedObject(view, @selector(finish), overlay, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    });
    dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
    return result;
  }
  CaptureJSON Capture(CaptureJSON request) {
    if (request.value("mode", "") == "region" && !request.contains("rect")) {
      auto selection = SelectRegion(); if (selection.value("cancelled",false)) return selection;
      request.update(selection);
    }
    if (@available(macOS 14.0, *)) {
      auto content = Content(); SCContentFilter* filter = nil;
      SCStreamConfiguration* config = [SCStreamConfiguration new]; config.showsCursor = NO; config.ignoreShadowsSingleWindow = YES;
      std::string id = request.value("sourceId", "");
      if (request.value("mode", "") == "window") {
        for (SCWindow* window in content.windows) if (std::to_string(window.windowID) == id) {
          filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:window];
          config.width = (size_t)ceil(filter.contentRect.size.width * filter.pointPixelScale);
          config.height = (size_t)ceil(filter.contentRect.size.height * filter.pointPixelScale);
        }
      } else {
        for (SCDisplay* display in content.displays) if (std::to_string(display.displayID) == id) {
          filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
          CGRect rect = CGRectMake(0,0,display.frame.size.width,display.frame.size.height);
          if (request.contains("rect")) {
            auto& r = request["rect"]; rect = CGRectMake(Coordinate(r,"x"),Coordinate(r,"y"),Coordinate(r,"width"),Coordinate(r,"height"));
            if (rect.size.width < 1 || rect.size.height < 1 || !CGRectContainsRect(CGRectMake(0,0,display.frame.size.width,display.frame.size.height),rect)) throw CaptureError("Select a region inside one monitor.");
          }
          config.sourceRect = rect;
          config.width = (size_t)ceil(rect.size.width * filter.pointPixelScale);
          config.height = (size_t)ceil(rect.size.height * filter.pointPixelScale);
        }
      }
      if (!filter) throw CaptureError("The selected screen or window is no longer available. Select it again.", "ERROR_REASON_NOT_FOUND", true);
      auto done = dispatch_semaphore_create(0); __block CGImageRef captured = nullptr;
      [SCScreenshotManager captureImageWithFilter:filter configuration:config completionHandler:^(CGImageRef image, NSError*) {
        if (image) captured = CGImageRetain(image); dispatch_semaphore_signal(done);
      }];
      dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
      if (!captured) throw CaptureError("Screen capture failed. Check permission and retry.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE", true);
      auto result = PNG(captured); CGImageRelease(captured); return result;
    }
    // Lynxtron supports macOS 12. Use the legacy OS memory API only below 14,
    // where SCScreenshotManager is unavailable. Resolve it dynamically because
    // current SDKs forbid linking this retired API. Remove this fallback when
    // Lane's documented minimum macOS version reaches 14.
    auto sources=LegacySources({{"mode",request.value("mode","")=="window"?"window":"screen"}})["sources"];
    std::string id=request.value("sourceId","");
    if(std::none_of(sources.begin(),sources.end(),[&](const auto& source){return source["id"]==id;}))throw CaptureError("Select an available source.","ERROR_REASON_NOT_FOUND",true);
    auto number=static_cast<uint32_t>(std::stoul(id));
    CGRect bounds=CGRectNull;CGWindowListOption options=kCGWindowListOptionIncludingWindow;CGWindowID window=number;
    if(request.value("mode","")!="window") {
      bounds=CGDisplayBounds(number);options=kCGWindowListOptionOnScreenOnly;window=kCGNullWindowID;
      if(request.contains("rect")){auto& r=request["rect"];CGRect local=CGRectMake(Coordinate(r,"x"),Coordinate(r,"y"),Coordinate(r,"width"),Coordinate(r,"height"));if(local.size.width<1||local.size.height<1||!CGRectContainsRect(CGRectMake(0,0,bounds.size.width,bounds.size.height),local))throw CaptureError("Select a region inside one monitor.");bounds=CGRectOffset(local,bounds.origin.x,bounds.origin.y);}
    }
    using LegacyCapture=CGImageRef(*)(CGRect,CGWindowListOption,CGWindowID,CGWindowImageOption);
    auto capture=reinterpret_cast<LegacyCapture>(dlsym(RTLD_DEFAULT,"CGWindowListCreateImage"));
    if(!capture)throw CaptureError("Screen capture is unavailable.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true);
    CGImageRef image=capture(bounds,options,window,kCGWindowImageBoundsIgnoreFraming|kCGWindowImageBestResolution);
    auto result=PNG(image);if(image)CGImageRelease(image);return result;
  }
  CaptureJSON Render(const CaptureJSON& request) {
    NSData* bytes = [[NSData alloc] initWithBase64EncodedString:Text(request.at("png").get<std::string>()) options:0];
    CGImageSourceRef source = bytes ? CGImageSourceCreateWithData((__bridge CFDataRef)bytes,nullptr) : nullptr;
    CGImageRef image = source ? CGImageSourceCreateImageAtIndex(source,0,nullptr) : nullptr;
    if (source) CFRelease(source);
    if (!image) throw CaptureError("The captured image is invalid.");
    size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
    try { ValidateEdits(request,width,height); } catch (...) { CGImageRelease(image); throw; }
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGContextRef context = CGBitmapContextCreate(nullptr,width,height,8,0,space,kCGImageAlphaPremultipliedLast);
    CGColorSpaceRelease(space);
    if (!context) { CGImageRelease(image); throw CaptureError("Not enough memory to edit this capture.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE",true); }
    CGContextDrawImage(context,CGRectMake(0,0,width,height),image); CGImageRelease(image);
    CGContextTranslateCTM(context,0,height); CGContextScaleCTM(context,1,-1);
    for (const auto& layer : request["layers"]) {
      auto type = layer.value("type","");
      double x=Coordinate(layer,"x"), y=Coordinate(layer,"y"), w=Coordinate(layer,"width"), h=Coordinate(layer,"height");
      CGRect rect = CGRectMake(x,y,w,h);
      auto color = std::stoul(layer.value("color","#2563EB").substr(1),nullptr,16);
      NSColor* ink = [NSColor colorWithSRGBRed:((color>>16)&255)/255.0 green:((color>>8)&255)/255.0 blue:(color&255)/255.0 alpha:1];
      CGContextSetStrokeColorWithColor(context,ink.CGColor); CGContextSetFillColorWithColor(context,ink.CGColor);
      CGContextSetLineWidth(context,Coordinate(layer,"strokeWidth",3)); CGContextSetLineCap(context,kCGLineCapRound);
      if (type == "cover") CGContextFillRect(context,rect);
      else if (type == "rectangle") CGContextStrokeRect(context,rect);
      else if (type == "text") {
        [NSGraphicsContext saveGraphicsState];
        [NSGraphicsContext setCurrentContext:[NSGraphicsContext graphicsContextWithCGContext:context flipped:YES]];
        [Text(layer.value("text","")) drawAtPoint:NSMakePoint(x,y) withAttributes:@{NSFontAttributeName:[NSFont systemFontOfSize:Coordinate(layer,"fontSize",20)], NSForegroundColorAttributeName:ink}];
        [NSGraphicsContext restoreGraphicsState];
      } else if (type == "blur" && w > 0 && h > 0) {
        CGImageRef current = CGBitmapContextCreateImage(context);
        CIImage* original = [CIImage imageWithCGImage:current];
        CIImage* blurred = [[original imageByClampingToExtent] imageByApplyingFilter:@"CIGaussianBlur" withInputParameters:@{kCIInputRadiusKey:@12}];
        CIContext* ci = [CIContext contextWithOptions:nil];
        CGImageRef patch = [ci createCGImage:blurred fromRect:CGRectMake(x,height-y-h,w,h)];
        if (patch) {
          CGContextSaveGState(context); CGContextTranslateCTM(context,x,y+h); CGContextScaleCTM(context,1,-1);
          CGContextDrawImage(context,CGRectMake(0,0,w,h),patch); CGContextRestoreGState(context); CGImageRelease(patch);
        }
        CGImageRelease(current);
      } else if (type == "draw" || type == "arrow") {
        auto points = layer.value("points",CaptureJSON::array());
        if (points.empty()) points = CaptureJSON::array({{{"x",x},{"y",y}},{{"x",x+w},{"y",y+h}}});
        CGContextBeginPath(context);
        for (size_t i=0;i<points.size();i++) {
          double px=Coordinate(points[i],"x"), py=Coordinate(points[i],"y");
          if (i==0) CGContextMoveToPoint(context,px,py); else CGContextAddLineToPoint(context,px,py);
        }
        CGContextStrokePath(context);
        if (type == "arrow" && points.size()>=2) {
          auto a=points[points.size()-2], b=points.back();
          double bx=Coordinate(b,"x"), by=Coordinate(b,"y"), angle=atan2(by-Coordinate(a,"y"),bx-Coordinate(a,"x"));
          double length=std::max(12.0,Coordinate(layer,"strokeWidth",3)*4);
          CGContextBeginPath(context); CGContextMoveToPoint(context,bx-length*cos(angle-0.5),by-length*sin(angle-0.5));
          CGContextAddLineToPoint(context,bx,by); CGContextAddLineToPoint(context,bx-length*cos(angle+0.5),by-length*sin(angle+0.5)); CGContextStrokePath(context);
        }
      }
    }
    CGImageRef final = CGBitmapContextCreateImage(context); CGContextRelease(context);
    if (request.contains("crop") && !request["crop"].is_null()) {
      const auto& c=request["crop"];
      CGImageRef cropped=CGImageCreateWithImageInRect(final,CGRectMake(Coordinate(c,"x"),Coordinate(c,"y"),Coordinate(c,"width"),Coordinate(c,"height")));
      CGImageRelease(final); final=cropped;
    }
    auto result=PNG(final); CGImageRelease(final); return result;
  }
  CaptureJSON Shortcut(const CaptureJSON& request) {
    std::string mode=request.at("mode"), accelerator=request.at("accelerator");
    __block bool accepted=false;
    Main(^{
      auto old=shortcuts.find(mode);
      if (accelerator.empty()) {
        if (old!=shortcuts.end()) { UnregisterEventHotKey(old->second); shortcuts.erase(old); }
        for(auto it=modes.begin();it!=modes.end();) { if(it->second==mode) it=modes.erase(it); else ++it; }
        accepted=true; return;
      }
      UInt32 modifiers=0; std::string key, token;
      std::istringstream stream(accelerator);
      while (std::getline(stream,token,'+')) {
        if (token=="Command" || token=="Cmd" || token=="Meta") modifiers|=cmdKey;
        else if (token=="Ctrl" || token=="Control") modifiers|=controlKey;
        else if (token=="Alt" || token=="Option") modifiers|=optionKey;
        else if (token=="Shift") modifiers|=shiftKey;
        else key=token;
      }
      const std::map<std::string,UInt32> keys={{"A",0},{"S",1},{"D",2},{"F",3},{"H",4},{"G",5},{"Z",6},{"X",7},{"C",8},{"V",9},{"B",11},{"Q",12},{"W",13},{"E",14},{"R",15},{"Y",16},{"T",17},{"1",18},{"2",19},{"3",20},{"4",21},{"6",22},{"5",23},{"9",25},{"7",26},{"8",28},{"0",29},{"O",31},{"U",32},{"I",34},{"P",35},{"L",37},{"J",38},{"K",40}};
      if (!modifiers || !keys.count(key)) return;
      if (modifiers == cmdKey && (key == "Q" || key == "O" || key == "K")) return;
      CFArrayRef systemKeys = nullptr;
      if (CopySymbolicHotKeys(&systemKeys) == noErr && systemKeys) {
        bool reserved = false;
        for (NSDictionary* binding in (__bridge NSArray*)systemKeys) {
          if ([binding[(__bridge NSString*)kHISymbolicHotKeyEnabled] boolValue] &&
              [binding[(__bridge NSString*)kHISymbolicHotKeyCode] unsignedIntValue] == keys.at(key) &&
              [binding[(__bridge NSString*)kHISymbolicHotKeyModifiers] unsignedIntValue] == modifiers) reserved = true;
        }
        CFRelease(systemKeys);
        if (reserved) return;
      }
      EventHotKeyRef fresh=nullptr; UInt32 id=next++;
      if (RegisterEventHotKey(keys.at(key),modifiers,EventHotKeyID{'LQAC',id},GetApplicationEventTarget(),kEventHotKeyExclusive,&fresh)!=noErr) return;
      if (old!=shortcuts.end()) UnregisterEventHotKey(old->second);
      for(auto it=modes.begin();it!=modes.end();) { if(it->second==mode) it=modes.erase(it); else ++it; }
      shortcuts[mode]=fresh; modes[id]=mode; accepted=true;
    });
    if (!accepted) throw CaptureError("That shortcut is unavailable or already used. Choose another combination; the previous shortcut is retained.","ERROR_REASON_CONFLICT");
    return CaptureJSON::object();
  }
  CaptureJSON Call(const CaptureJSON& request) {
    auto op=request.value("op","");
    if (op=="cancel") { Main(^{ if(selectionView) selectionView.finish(NO); }); return CaptureJSON::object(); }
    if (op=="sources") return Sources(request);
    if (op=="capture") return Capture(request);
    if (op=="render") return Render(request);
    if (op=="shortcut") return Shortcut(request);
    if (op=="permission") return {{"permission",CGPreflightScreenCaptureAccess()?"granted":"not_granted"}};
    if (op=="openPermissionSettings") { Main(^{ [NSWorkspace.sharedWorkspace openURL:[NSURL URLWithString:@"x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"]]; }); return CaptureJSON::object(); }
    if (op=="activeBrowser") { __block bool chrome=false; Main(^{ chrome=[NSWorkspace.sharedWorkspace.frontmostApplication.bundleIdentifier isEqualToString:@"com.google.Chrome"]; }); return {{"chrome",(bool)chrome}}; }
    throw CaptureError("Unknown capture operation.");
  }
};
}
std::shared_ptr<Handler> CreateCaptureHost(std::shared_ptr<Events> events) {
  auto host=std::make_shared<MacCapture>(std::move(events));
  return std::make_shared<Handler>([host](const auto&,const auto& args) {
    @autoreleasepool {
      const auto started=std::chrono::steady_clock::now();
      std::string operation="unknown";
      const auto log=[&](bool ok) {
        auto elapsed=std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now()-started).count();
        LaneLog(ok?"lane_realqa_operation_completed":"lane_realqa_operation_failed", operation.c_str(),elapsed,ok?"completed":"failed");
      };
      try {
        auto request=CaptureJSON::parse(args.at(0));
        auto candidate=request.value("op","");
        if(candidate=="capture"||candidate=="sources"||candidate=="render"||candidate=="shortcut"||candidate=="permission"||candidate=="cancel")operation=candidate;
        auto result=host->Call(request);log(true);return CaptureJSON({{"ok",true},{"value",result}}).dump();
      }
      catch (const CaptureError& error) { log(false); return CaptureFailure(error).dump(); }
      catch (...) { log(false); return CaptureFailure(CaptureError("Capture operation failed. Retry without discarding the draft.","ERROR_REASON_DEPENDENCY_UNAVAILABLE",true)).dump(); }
    }
  });
}
}
