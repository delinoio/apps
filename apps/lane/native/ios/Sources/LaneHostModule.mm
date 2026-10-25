// SPDX-License-Identifier: Apache-2.0
#import "LaneHost.h"
#import "LaneTransport.h"
#import <UIKit/UIKit.h>

static NSString *HostResult(BOOL ok, NSString *reason) {
  return ok ? LaneEncode(@{@"ok": @YES}) : LaneFailure(reason, NO);
}
@implementation LaneHostModule
+ (NSString *)name { return @"LaneHost"; }
+ (NSDictionary *)methodLookup {
  return @{@"copyText": NSStringFromSelector(@selector(copyText:callback:)),
           @"cancelReauthentication": NSStringFromSelector(@selector(cancelReauthentication:callback:)),
           @"openURL": NSStringFromSelector(@selector(openURL:callback:))};
}
- (void)cancelReauthentication:(NSString *)challenge callback:(LynxCallbackBlock)callback {
  [[LaneHost shared] cancelReauthentication:challenge completion:callback];
}
- (void)copyText:(NSString *)text callback:(LynxCallbackBlock)callback {
  dispatch_async(dispatch_get_main_queue(), ^{
    if (![text isKindOfClass:NSString.class]) { callback(HostResult(NO, @"INVALID_INPUT")); return; }
    UIPasteboard.generalPasteboard.string = text;
    callback(HostResult(YES, nil));
  });
}
- (void)openURL:(NSString *)text callback:(LynxCallbackBlock)callback {
  dispatch_async(dispatch_get_main_queue(), ^{
    NSURL *url = [text isKindOfClass:NSString.class] ? [NSURL URLWithString:text] : nil;
    if (![url.scheme.lowercaseString isEqualToString:@"https"] || !url.host.length || url.user || url.password) {
      callback(HostResult(NO, @"INVALID_INPUT")); return;
    }
    [UIApplication.sharedApplication openURL:url options:@{} completionHandler:^(BOOL opened) {
      callback(HostResult(opened, @"DEPENDENCY_UNAVAILABLE"));
    }];
  });
}
@end
