// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
#import <Lynx/LynxModule.h>
#import <Lynx/LynxView.h>

NS_ASSUME_NONNULL_BEGIN
/// The iOS implementation of the AnyDashHost contract (src/host.ts): Logto
/// PKCE sign-in, Keychain session storage, API requests, the persisted widget
/// cache and WidgetKit snapshots in the app group. The app is a viewer only.
@interface AnyDashHost : NSObject
@property(nonatomic, weak, nullable) LynxView *view;
+ (instancetype)shared;
- (void)call:(NSString *)method payload:(NSString *)payload completion:(void (^)(NSString *reply))completion;
- (BOOL)handleURL:(NSURL *)url;
- (void)setVisible:(BOOL)visible;
@end

@interface AnyDashHostModule : NSObject <LynxModule>
@end
NS_ASSUME_NONNULL_END
