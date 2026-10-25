// SPDX-License-Identifier: Apache-2.0
#import <UIKit/UIKit.h>
#import "LaneHost.h"
#import <XElement/LynxUIInput.h>
#import <XElement/LynxUITextArea.h>
#import "LaneLog.h"
#import <Lynx/LynxConfig.h>
#import <Lynx/LynxEnv.h>
#import <Lynx/LynxTemplateProvider.h>
#import <Lynx/LynxView.h>
#import <Lynx/LynxImageFetcher.h>

@interface LaneProvider : NSObject <LynxTemplateProvider>
@end
@implementation LaneProvider
- (void)loadTemplateWithUrl:(NSString *)url onComplete:(LynxTemplateLoadBlock)callback {
  if (![url isEqualToString:@"main.lynx"]) {
    callback(nil, [NSError errorWithDomain:@"io.delino.lane" code:1 userInfo:nil]);
    return;
  }
  NSError *error = nil;
  NSURL *resource = [[NSBundle mainBundle] URLForResource:@"main.lynx" withExtension:@"bundle"];
  NSData *data = resource ? [NSData dataWithContentsOfURL:resource options:0 error:&error] : nil;
  if (!data) {
    LaneLog(@"lane_template_failed");
    error = error ?: [NSError errorWithDomain:@"io.delino.lane" code:2 userInfo:nil];
  }
  callback(data, error);
}
@end

// The host resolves and validates attachment ownership before returning inline
// PNGs. Render those bytes without a filesystem path or a shared image cache.
@interface LaneImageFetcher : NSObject <LynxImageFetcher>
@end
@implementation LaneImageFetcher
- (dispatch_block_t)loadImageWithURL:(NSURL *)url size:(CGSize)size contextInfo:(NSDictionary *)context
    completion:(LynxImageLoadCompletionBlock)completion {
  NSProgress *progress = [NSProgress progressWithTotalUnitCount:1];
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    if (progress.cancelled) return;
    NSString *prefix = @"data:image/png;base64,";
    NSString *source = url.absoluteString;
    NSData *data = [source hasPrefix:prefix] ? [[NSData alloc] initWithBase64EncodedString:[source substringFromIndex:prefix.length] options:0] : nil;
    UIImage *image = data ? [UIImage imageWithData:data] : nil;
    dispatch_async(dispatch_get_main_queue(), ^{
      if (!progress.cancelled) completion(image, image ? nil : [NSError errorWithDomain:@"io.delino.lane.image" code:1 userInfo:nil], url);
    });
  });
  return ^{ [progress cancel]; };
}
@end

@interface LaneController : UIViewController
@property(nonatomic, strong) LynxView *lynxView;
@property(nonatomic, strong) LaneImageFetcher *images;
@property(nonatomic) CGRect keyboardFrame;
@end
@implementation LaneController
- (void)viewDidLoad {
  [super viewDidLoad];
  self.view.backgroundColor = UIColor.whiteColor;
  [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(keyboardChanged:) name:UIKeyboardWillChangeFrameNotification object:nil];
  [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(keyboardChanged:) name:UIKeyboardWillHideNotification object:nil];
  [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(updateTextSize) name:UIContentSizeCategoryDidChangeNotification object:nil];
  self.lynxView = [[LynxView alloc] initWithBuilderBlock:^(LynxViewBuilder *builder) {
    LynxConfig *config = [[LynxConfig alloc] initWithProvider:[LaneProvider new]];
    [config registerUI:LynxUIInput.class withName:@"input"];
    [config registerShadowNode:LynxUIInputShadowNode.class withName:@"input"];
    [config registerUI:LynxUITextArea.class withName:@"textarea"];
    [config registerShadowNode:LynxUITextAreaShadowNode.class withName:@"textarea"];
    [config registerModule:LaneStoreModule.class];
    [config registerModule:LaneHostModule.class];
    builder.config = config;
    builder.screenSize = self.view.safeAreaLayoutGuide.layoutFrame.size;
    builder.fontScale = [UIFontMetrics.defaultMetrics scaledValueForValue:1.0];
  }];
  self.images = [LaneImageFetcher new];
  self.lynxView.imageFetcher = self.images;
  self.lynxView.layoutWidthMode = LynxViewSizeModeExact;
  self.lynxView.layoutHeightMode = LynxViewSizeModeExact;
  [self.view addSubview:self.lynxView];
  [LaneHost shared].view = self.lynxView;
  [self.lynxView loadTemplateFromURL:@"main.lynx" initData:nil];
}
- (void)keyboardChanged:(NSNotification *)notification {
  self.keyboardFrame = [notification.name isEqual:UIKeyboardWillHideNotification] ? CGRectZero :
    [self.view convertRect:[notification.userInfo[UIKeyboardFrameEndUserInfoKey] CGRectValue] fromView:nil];
  [self.view setNeedsLayout];
  [self.view layoutIfNeeded];
}
- (void)updateTextSize { [self.lynxView updateFontScale:[UIFontMetrics.defaultMetrics scaledValueForValue:1.0]]; }
- (void)dealloc { [NSNotificationCenter.defaultCenter removeObserver:self]; }
- (void)viewDidAppear:(BOOL)animated {
  [super viewDidAppear:animated];
  [self.lynxView onEnterForeground];
}
- (void)viewDidDisappear:(BOOL)animated {
  [self.lynxView onEnterBackground];
  [super viewDidDisappear:animated];
}
- (void)viewDidLayoutSubviews {
  [super viewDidLayoutSubviews];
  CGRect contentFrame = self.view.safeAreaLayoutGuide.layoutFrame;
  // Keep the editor footer usable with a docked keyboard. Floating keyboards
  // leave the full safe-area viewport available for normal scrolling.
  CGRect keyboard = CGRectIntersection(self.view.bounds, self.keyboardFrame);
  if (!CGRectIsNull(keyboard) && keyboard.size.width >= contentFrame.size.width && keyboard.size.height > 0)
    contentFrame.size.height = MAX(0, MIN(CGRectGetMaxY(contentFrame), CGRectGetMinY(keyboard)) - contentFrame.origin.y);
  BOOL resized = !CGSizeEqualToSize(self.lynxView.bounds.size, contentFrame.size);
  self.lynxView.frame = contentFrame;
  self.lynxView.preferredLayoutWidth = contentFrame.size.width;
  self.lynxView.preferredLayoutHeight = contentFrame.size.height;
  if (resized) [self.lynxView updateScreenMetricsWithWidth:contentFrame.size.width height:contentFrame.size.height];
  [self.lynxView triggerLayout];
}
@end

@interface LaneDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end
@implementation LaneDelegate
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
  [LynxEnv sharedInstance];
  [[LaneHost shared] start];
  NSURL *launchURL = options[UIApplicationLaunchOptionsURLKey];
  if (launchURL) [[LaneHost shared] handleURL:launchURL];
  self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
  self.window.rootViewController = [LaneController new];
  [self.window makeKeyAndVisible];
  LaneLog(@"lane_started");
  return YES;
}
- (void)applicationDidEnterBackground:(UIApplication *)application {
  [[LaneHost shared] background];
  LaneController *controller = (LaneController *)self.window.rootViewController;
  [controller.lynxView onEnterBackground];
}
- (void)applicationWillEnterForeground:(UIApplication *)application {
  [[LaneHost shared] foreground];
  LaneController *controller = (LaneController *)self.window.rootViewController;
  [controller.lynxView onEnterForeground];
}
- (void)applicationDidBecomeActive:(UIApplication *)application {
  [[LaneHost shared] foreground];
}
- (BOOL)application:(UIApplication *)application openURL:(NSURL *)url options:(NSDictionary *)options {
  return [[LaneHost shared] handleURL:url];
}
- (void)application:(UIApplication *)application didRegisterForRemoteNotificationsWithDeviceToken:(NSData *)token {
  [[LaneHost shared] registeredPushToken:token];
}
- (void)application:(UIApplication *)application didFailToRegisterForRemoteNotificationsWithError:(NSError *)error {
  [[LaneHost shared] pushRegistrationFailed];
}
- (void)application:(UIApplication *)application didReceiveRemoteNotification:(NSDictionary *)payload
    fetchCompletionHandler:(void (^)(UIBackgroundFetchResult))completion {
  [[LaneHost shared] receivePush:payload completion:^(BOOL success) {
    completion(success ? UIBackgroundFetchResultNewData : UIBackgroundFetchResultFailed);
  }];
}

@end
int main(int argc, char *argv[]) {
  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass(LaneDelegate.class)); }
}
