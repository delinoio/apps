// SPDX-License-Identifier: Apache-2.0
#pragma once
#import <Cocoa/Cocoa.h>
#import <UserNotifications/UserNotifications.h>
#include "../handler.h"

@interface LaneRuntime : NSObject
- (instancetype)initWithHelperURL:(NSURL*)helperURL scheme:(NSString*)scheme;
@property(nonatomic, copy) void (^eventSink)(NSString*, NSArray*);
- (std::shared_ptr<lane::desktop::Handler>)storeHandler;
- (std::shared_ptr<lane::desktop::Handler>)hostHandler;
- (void)resume;
- (void)viewClosed;
- (void)handleURL:(NSURL*)url;
- (void)handleNotification:(NSDictionary*)info;
- (void)stop;
- (void)stopWithCompletion:(dispatch_block_t)completion;
- (void)receivePush:(NSDictionary*)payload;
- (void)registerPushToken:(NSData*)token;
#ifdef LANE_NATIVE_TESTING
// Native fixtures replace the wall clock and shorten the registration retry
// delay; production builds always use the system clock and 30 seconds.
- (void)setPushClock:(NSDate* (^)(void))clock retryDelay:(NSTimeInterval)delay;
// Waits until no APNs registration helper call is outstanding.
- (BOOL)waitForPushIdle:(NSTimeInterval)timeout;
#endif
@end
