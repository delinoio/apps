// SPDX-License-Identifier: Apache-2.0
#pragma once
#import <Cocoa/Cocoa.h>
#import <UserNotifications/UserNotifications.h>
#include "../async_module.h"

@interface LaneRuntime : NSObject
@property(nonatomic, copy) void (^eventSink)(NSString*, NSArray*);
- (std::shared_ptr<lane::desktop::Handler>)storeHandler;
- (std::shared_ptr<lane::desktop::Handler>)hostHandler;
- (void)resume;
- (void)handleURL:(NSURL*)url;
- (void)stop;
- (void)registerPushToken:(NSData*)token;
@end
