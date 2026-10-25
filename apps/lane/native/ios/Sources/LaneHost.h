// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
#import <Lynx/LynxModule.h>
#import <Lynx/LynxView.h>
#import <UserNotifications/UserNotifications.h>

NS_ASSUME_NONNULL_BEGIN
// The application owns this service. A Lynx module only owns its subscriptions.
@interface LaneHost : NSObject <UNUserNotificationCenterDelegate>
@property(nonatomic, weak, nullable) LynxView *view;
+ (instancetype)shared;
- (void)start;
- (void)foreground;
- (void)background;
- (void)open:(NSString *)json completion:(LynxCallbackBlock)completion;
- (void)invoke:(NSString *)method json:(NSString *)json completion:(LynxCallbackBlock)completion;
- (void)close:(NSString *)space completion:(LynxCallbackBlock)completion;
- (BOOL)handleURL:(NSURL *)url;
- (void)registeredPushToken:(NSData *)token;
- (void)pushRegistrationFailed;
- (void)receivePush:(NSDictionary *)payload completion:(void (^)(BOOL))completion;
@end

@interface LaneStoreModule : NSObject <LynxModule>
@end
NS_ASSUME_NONNULL_END

@interface LaneHostModule : NSObject <LynxModule>
@end
