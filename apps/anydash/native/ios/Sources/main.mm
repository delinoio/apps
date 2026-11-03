// SPDX-License-Identifier: Apache-2.0
#import <UIKit/UIKit.h>
#import "AnyDashHost.h"
#import <XElement/LynxUIInput.h>
#import <Lynx/LynxConfig.h>
#import <Lynx/LynxEnv.h>
#import <Lynx/LynxTemplateData.h>
#import <Lynx/LynxTemplateProvider.h>
#import <Lynx/LynxView.h>
#import <Lynx/LynxViewClient.h>

@interface AnyDashProvider : NSObject <LynxTemplateProvider>
@end
@implementation AnyDashProvider
- (void)loadTemplateWithUrl:(NSString *)url onComplete:(LynxTemplateLoadBlock)callback {
  NSURL *resource = [url isEqualToString:@"main.lynx"] ? [NSBundle.mainBundle URLForResource:@"main.lynx" withExtension:@"bundle"] : nil;
  NSData *data = resource ? [NSData dataWithContentsOfURL:resource] : nil;
  callback(data, data ? nil : [NSError errorWithDomain:@"io.delino.anydash" code:1 userInfo:nil]);
}
@end

/// Structured logs for template load and runtime errors; codes only, no content.
@interface AnyDashLifecycle : NSObject <LynxViewLifecycle>
@end
@implementation AnyDashLifecycle
- (void)lynxView:(LynxView *)view didLoadFinishedWithUrl:(NSString *)url {
  NSLog(@"{\"event\":\"anydash_template_loaded\",\"platform\":\"ios\"}");
}
- (void)lynxView:(LynxView *)view didRecieveError:(NSError *)error {
  NSLog(@"{\"event\":\"anydash_lynx_error\",\"platform\":\"ios\",\"code\":%ld}", (long)error.code);
#if DEBUG
  NSLog(@"anydash_lynx_error_detail %@", error.userInfo);
#endif
}
@end

@interface AnyDashController : UIViewController
@property(nonatomic, strong) LynxView *lynxView;
@property(nonatomic, copy) NSString *initialPath;
@property(nonatomic, strong) AnyDashLifecycle *lifecycle;
@end
@implementation AnyDashController
- (void)viewDidLoad {
  [super viewDidLoad];
  self.lifecycle = [AnyDashLifecycle new];
  self.view.backgroundColor = UIColor.systemBackgroundColor;
  self.lynxView = [[LynxView alloc] initWithBuilderBlock:^(LynxViewBuilder *builder) {
    LynxConfig *config = [[LynxConfig alloc] initWithProvider:[AnyDashProvider new]];
    [config registerUI:LynxUIInput.class withName:@"input"];
    [config registerShadowNode:LynxUIInputShadowNode.class withName:@"input"];
    [config registerModule:AnyDashHostModule.class];
    builder.config = config;
    builder.screenSize = self.view.safeAreaLayoutGuide.layoutFrame.size;
    builder.fontScale = [UIFontMetrics.defaultMetrics scaledValueForValue:1.0];
  }];
  [self.lynxView addLifecycleClient:self.lifecycle];
  self.lynxView.layoutWidthMode = LynxViewSizeModeExact;
  self.lynxView.layoutHeightMode = LynxViewSizeModeExact;
  BOOL dark = self.traitCollection.userInterfaceStyle == UIUserInterfaceStyleDark;
  [self.lynxView updateGlobalPropsWithDictionary:@{@"initialPath": self.initialPath ?: @"/", @"theme": dark ? @"dark" : @"light"}];
  [self.view addSubview:self.lynxView];
  [AnyDashHost shared].view = self.lynxView;
  [self.lynxView loadTemplateFromURL:@"main.lynx" initData:nil];
}
- (void)viewDidLayoutSubviews {
  [super viewDidLayoutSubviews];
  CGRect frame = self.view.safeAreaLayoutGuide.layoutFrame;
  BOOL resized = !CGSizeEqualToSize(self.lynxView.bounds.size, frame.size);
  self.lynxView.frame = frame;
  self.lynxView.preferredLayoutWidth = frame.size.width;
  self.lynxView.preferredLayoutHeight = frame.size.height;
  // iPad split view and rotation switch between the phone and tablet layouts.
  if (resized) [self.lynxView updateScreenMetricsWithWidth:frame.size.width height:frame.size.height];
  [self.lynxView triggerLayout];
}
@end

@interface AnyDashDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end
@implementation AnyDashDelegate
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
  [LynxEnv sharedInstance];
  AnyDashController *controller = [AnyDashController new];
  NSURL *launch = options[UIApplicationLaunchOptionsURLKey];
  // Native widgets open anydash://spaces/<space>/dashboards/<dashboard>?widget=<widget>.
  if ([launch.scheme isEqualToString:@"anydash"] && ![launch.host isEqualToString:@"auth"]) controller.initialPath = launch.absoluteString;
  self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
  self.window.rootViewController = controller;
  [self.window makeKeyAndVisible];
  NSLog(@"{\"event\":\"anydash_started\",\"platform\":\"ios\"}");
  return YES;
}
- (void)applicationDidEnterBackground:(UIApplication *)application {
  [[AnyDashHost shared] setVisible:NO];
  [((AnyDashController *)self.window.rootViewController).lynxView onEnterBackground];
}
- (void)applicationWillEnterForeground:(UIApplication *)application {
  [((AnyDashController *)self.window.rootViewController).lynxView onEnterForeground];
  [[AnyDashHost shared] setVisible:YES];
}
- (BOOL)application:(UIApplication *)application openURL:(NSURL *)url options:(NSDictionary *)options {
  return [[AnyDashHost shared] handleURL:url];
}
@end

int main(int argc, char *argv[]) {
  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass(AnyDashDelegate.class)); }
}
