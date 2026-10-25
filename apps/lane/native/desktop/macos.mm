// SPDX-License-Identifier: Apache-2.0
#import <Cocoa/Cocoa.h>
#include "lane.h"
#import "macos/runtime.h"

@interface LaneDelegate : NSObject <NSApplicationDelegate, NSWindowDelegate, UNUserNotificationCenterDelegate>
@property(nonatomic, strong) NSWindow* window;
@property(nonatomic, strong) NSMutableArray<NSURL*>* pendingURLs;
@end
@implementation LaneDelegate {
  std::unique_ptr<lynx::pub::LynxView> _lynxView;
  std::shared_ptr<LaneClient> _client;
  LaneRuntime* _runtime;
  std::shared_ptr<lane::desktop::Handler> _storeHandler, _hostHandler;
}
- (void)applicationDidFinishLaunching:(NSNotification*)notification {
  _runtime = [LaneRuntime new];
  _storeHandler = [_runtime storeHandler]; _hostHandler = [_runtime hostHandler];
  __weak LaneDelegate* weak = self;
  _runtime.eventSink = ^(NSString* name, NSArray* arguments) {
    LaneDelegate* owner = weak;
    if (!owner || !owner->_lynxView) return;
    NSData* data = [NSJSONSerialization dataWithJSONObject:arguments options:0 error:nil];
    if (data) owner->_lynxView->SendGlobalEvent(name.UTF8String, std::string(static_cast<const char*>(data.bytes), data.length));
  };
  [UNUserNotificationCenter currentNotificationCenter].delegate = self;
  NSMenu* menu = [NSMenu new]; NSMenuItem* item = [NSMenuItem new];
  [menu addItem:item]; NSMenu* applicationMenu = [NSMenu new];
  [applicationMenu addItemWithTitle:@"Open Lane" action:@selector(openWindow) keyEquivalent:@"o"];
  [applicationMenu addItemWithTitle:@"Quit Lane" action:@selector(terminate:) keyEquivalent:@"q"];
  item.submenu = applicationMenu; NSApp.mainMenu = menu;
  [self openWindow];
  for (NSURL* url in self.pendingURLs) [_runtime handleURL:url];
  self.pendingURLs = nil;
#ifdef LANE_NATIVE_TESTING
  if ([NSProcessInfo.processInfo.environment[@"LANE_NATIVE_TEST_LIFECYCLE"] isEqualToString:@"1"]) {
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2*NSEC_PER_SEC), dispatch_get_main_queue(), ^{
      [self.window performClose:nil];
      fprintf(stderr, "LANE_NATIVE_WINDOW_CLOSED\n");
      dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2*NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [self openWindow];
        fprintf(stderr, "LANE_NATIVE_WINDOW_REOPENED\n");
      });
    });
  }
#endif
}
- (void)openWindow {
  if (_lynxView) { [self.window makeKeyAndOrderFront:nil]; return; }
  NSString* resources = NSBundle.mainBundle.resourcePath;
  auto& env = lynx::pub::LynxEnv::GetInstance();
  env.SetICUDataPath([[resources stringByAppendingPathComponent:@"icudtl.dat"] UTF8String]);
  self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1280, 820)
    styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
              NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
    backing:NSBackingStoreBuffered defer:NO];
  self.window.releasedWhenClosed = NO;
  self.window.title = @"Lane";
  self.window.delegate = self;
  [self.window center];
  const auto size = self.window.contentView.bounds.size;
  lynx::pub::LynxView::Builder builder;
  builder.SetScreenSize(size.width, size.height, self.window.backingScaleFactor)
    .SetFrame(0, 0, size.width, size.height)
    .SetParent((__bridge NativeWindow)self.window.contentView);
  builder.RegisterNativeModule("LaneStore", lane::desktop::StoreModule, &_storeHandler);
  builder.RegisterNativeModule("LaneHost", lane::desktop::HostModule, &_hostHandler);
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
  [_runtime resume];
}
- (void)windowDidResignKey:(NSNotification*)notification {
  if (_lynxView) _lynxView->OnEnterBackground();
}
- (void)windowWillClose:(NSNotification*)notification { [_runtime viewClosed]; _lynxView.reset(); }
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication*)sender { return NO; }
- (BOOL)applicationShouldHandleReopen:(NSApplication*)app hasVisibleWindows:(BOOL)visible {
  [self openWindow]; [_runtime resume]; return YES;
}
- (void)application:(NSApplication*)app openURLs:(NSArray<NSURL*>*)urls {
  if (!_runtime) {
    if (!self.pendingURLs) self.pendingURLs = [NSMutableArray new];
    [self.pendingURLs addObjectsFromArray:urls]; return;
  }
  [self openWindow];
  for (NSURL* url in urls) [_runtime handleURL:url];
}
- (void)application:(NSApplication*)app didRegisterForRemoteNotificationsWithDeviceToken:(NSData*)token {
  [_runtime registerPushToken:token];
}
- (void)application:(NSApplication*)app didFailToRegisterForRemoteNotificationsWithError:(NSError*)error {
  NSLog(@"{\"event\":\"lane_push_unavailable\"}");
}
- (void)application:(NSApplication*)app didReceiveRemoteNotification:(NSDictionary*)payload {
  [_runtime receivePush:payload];
}
- (void)userNotificationCenter:(UNUserNotificationCenter*)center willPresentNotification:(UNNotification*)notification
  withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
  completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionSound);
}
- (void)userNotificationCenter:(UNUserNotificationCenter*)center didReceiveNotificationResponse:(UNNotificationResponse*)response
  withCompletionHandler:(void (^)(void))completion {
  [self openWindow];
  NSDictionary* info = response.notification.request.content.userInfo;
  [_runtime handleNotification:info];
  completion();
}
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication*)app {
  [_runtime stopWithCompletion:^{ [NSApp replyToApplicationShouldTerminate:YES]; }];
  return NSTerminateLater;
}
- (void)applicationWillTerminate:(NSNotification*)notification { [_runtime stop]; _lynxView.reset(); }
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
