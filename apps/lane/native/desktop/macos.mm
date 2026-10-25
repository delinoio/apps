// SPDX-License-Identifier: Apache-2.0
#import <Cocoa/Cocoa.h>
#include "lane.h"

@interface LaneDelegate : NSObject <NSApplicationDelegate, NSWindowDelegate>
@property(nonatomic, strong) NSWindow* window;
@end
@implementation LaneDelegate {
  std::unique_ptr<lynx::pub::LynxView> _lynxView;
  std::shared_ptr<LaneClient> _client;
}
- (void)applicationDidFinishLaunching:(NSNotification*)notification {
  NSString* resources = NSBundle.mainBundle.resourcePath;
  auto& env = lynx::pub::LynxEnv::GetInstance();
  env.SetICUDataPath([[resources stringByAppendingPathComponent:@"icudtl.dat"] UTF8String]);
  self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1280, 820)
    styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
              NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
    backing:NSBackingStoreBuffered defer:NO];
  self.window.title = @"Lane";
  self.window.delegate = self;
  [self.window center];
  const auto size = self.window.contentView.bounds.size;
  lynx::pub::LynxView::Builder builder;
  builder.SetScreenSize(size.width, size.height, self.window.backingScaleFactor)
    .SetFrame(0, 0, size.width, size.height)
    .SetParent((__bridge NativeWindow)self.window.contentView);
  _lynxView = builder.Build();
  _client = std::make_shared<LaneClient>();
  _lynxView->AddClient(_client);
  try {
    LoadLane(*_lynxView, [[resources stringByAppendingPathComponent:@"main.lynx.bundle"] UTF8String]);
  } catch (const std::exception&) {
    std::cerr << "{\"event\":\"lane_start_failed\"}\n";
    [NSApp terminate:nil];
    return;
  }
  [self.window makeKeyAndOrderFront:nil];
  [NSApp activateIgnoringOtherApps:YES];
  std::clog << "{\"event\":\"lane_started\",\"platform\":\"macos\"}\n";
}
- (void)windowDidResize:(NSNotification*)notification {
  if (_lynxView) {
    const auto size = self.window.contentView.bounds.size;
    _lynxView->SetFrame(0, 0, size.width, size.height);
  }
}
- (void)windowDidBecomeKey:(NSNotification*)notification {
  if (_lynxView) _lynxView->OnEnterForeground();
}
- (void)windowDidResignKey:(NSNotification*)notification {
  if (_lynxView) _lynxView->OnEnterBackground();
}
- (void)windowWillClose:(NSNotification*)notification { _lynxView.reset(); }
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication*)sender { return YES; }
- (void)applicationWillTerminate:(NSNotification*)notification { _lynxView.reset(); }
@end
int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    LaneDelegate* delegate = [LaneDelegate new];
    NSApp.delegate = delegate;
    [NSApp run];
  }
  return 0;
}
