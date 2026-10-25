// SPDX-License-Identifier: Apache-2.0
#import "runtime.h"
#include "../platform_runtime.h"

// Forward all existing application delegate selectors to Lynxtron. Only the
// Lane APNs callbacks are intercepted; Lynxtron still owns windows and lifecycle.
@interface LaneDelegateProxy : NSObject <NSApplicationDelegate, UNUserNotificationCenterDelegate>
@property(nonatomic, strong) id previous;
@property(nonatomic, strong) id previousNotifications;
@property(nonatomic, strong) LaneRuntime* runtime;
@property(nonatomic, copy) void (^activate)(void);
@end
@implementation LaneDelegateProxy
- (BOOL)respondsToSelector:(SEL)selector {
  return [super respondsToSelector:selector] || [self.previous respondsToSelector:selector];
}
- (id)forwardingTargetForSelector:(SEL)selector { return self.previous; }
- (void)application:(NSApplication*)app didRegisterForRemoteNotificationsWithDeviceToken:(NSData*)token {
  [self.runtime registerPushToken:token];
}
- (void)application:(NSApplication*)app didFailToRegisterForRemoteNotificationsWithError:(NSError*)error {
  NSLog(@"{\"event\":\"lane_push_unavailable\"}");
}
- (void)application:(NSApplication*)app didReceiveRemoteNotification:(NSDictionary*)payload {
  [self.runtime receivePush:payload];
}
- (void)userNotificationCenter:(UNUserNotificationCenter*)center willPresentNotification:(UNNotification*)notification
    withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
  completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionSound);
}
- (void)userNotificationCenter:(UNUserNotificationCenter*)center didReceiveNotificationResponse:(UNNotificationResponse*)response
    withCompletionHandler:(void (^)(void))completion {
  self.activate();
  [self.runtime handleNotification:response.notification.request.content.userInfo];
  completion();
}
@end
namespace lane::desktop {
namespace {
struct MacRuntime : PlatformRuntime {
  LaneRuntime* runtime;
  LaneDelegateProxy* delegate;
  MacRuntime(const std::string& resources, const std::string& scheme, std::shared_ptr<Events> events) {
    NSString* path = [NSString stringWithUTF8String:resources.c_str()];
    runtime = [[LaneRuntime alloc] initWithHelperURL:[NSURL fileURLWithPath:[path stringByAppendingPathComponent:@"helpers/lane"]]
      scheme:[NSString stringWithUTF8String:scheme.c_str()]];
    runtime.eventSink = ^(NSString* name, NSArray* arguments) {
      NSData* bytes = [NSJSONSerialization dataWithJSONObject:arguments options:0 error:nil];
      if (bytes) events->Push(name.UTF8String, std::string(static_cast<const char*>(bytes.bytes), bytes.length));
    };
    delegate = [LaneDelegateProxy new];
    delegate.previous = NSApp.delegate;
    delegate.previousNotifications = UNUserNotificationCenter.currentNotificationCenter.delegate;
    delegate.runtime = runtime;
    delegate.activate = ^{ events->Push("lane.activate", "[]"); };
    NSApp.delegate = delegate;
    UNUserNotificationCenter.currentNotificationCenter.delegate = delegate;
    store = [runtime storeHandler]; host = [runtime hostHandler];
    LaneRuntime* service = runtime;
    control = std::make_shared<Handler>([service](const std::string&, const std::vector<std::string>& args) {
      const auto& method = args.at(0);
      if (method == "resume") [service resume];
      else if (method == "viewClosed") [service viewClosed];
      else if (method == "url") [service handleURL:[NSURL URLWithString:[NSString stringWithUTF8String:args.at(1).c_str()]]];
      else if (method == "stop") {
        auto done = dispatch_semaphore_create(0);
        [service stopWithCompletion:^{ dispatch_semaphore_signal(done); }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
      } else throw std::runtime_error("Unknown host control operation");
      return std::string("{\"ok\":true}");
    });
  }
  ~MacRuntime() {
    // Normal shutdown awaits stop through the asynchronous control binding.
    // Do not synchronously wait on the main queue during environment cleanup.
    [runtime stop];
    if (NSApp.delegate == delegate) NSApp.delegate = delegate.previous;
    if (UNUserNotificationCenter.currentNotificationCenter.delegate == delegate)
      UNUserNotificationCenter.currentNotificationCenter.delegate = delegate.previousNotifications;
  }
};
}
std::string ReadNotificationActivation(const std::string&) { return {}; }
std::shared_ptr<PlatformRuntime> CreatePlatformRuntime(
    const std::string& resources, const std::string& scheme, std::shared_ptr<Events> events) {
  return std::make_shared<MacRuntime>(resources, scheme, std::move(events));
}
}
