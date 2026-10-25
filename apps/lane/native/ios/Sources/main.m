// SPDX-License-Identifier: Apache-2.0
#import <UIKit/UIKit.h>
#import <Lynx/LynxConfig.h>
#import <Lynx/LynxEnv.h>
#import <Lynx/LynxTemplateProvider.h>
#import <Lynx/LynxView.h>

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
    NSLog(@"{\"event\":\"lane_template_failed\",\"platform\":\"ios\"}");
    error = error ?: [NSError errorWithDomain:@"io.delino.lane" code:2 userInfo:nil];
  }
  callback(data, error);
}
@end

@interface LaneController : UIViewController
@property(nonatomic, strong) LynxView *lynxView;
@end
@implementation LaneController
- (void)viewDidLoad {
  [super viewDidLoad];
  self.view.backgroundColor = UIColor.whiteColor;
  self.lynxView = [[LynxView alloc] initWithBuilderBlock:^(LynxViewBuilder *builder) {
    builder.config = [[LynxConfig alloc] initWithProvider:[LaneProvider new]];
    builder.screenSize = self.view.bounds.size;
    builder.fontScale = 1.0;
  }];
  self.lynxView.layoutWidthMode = LynxViewSizeModeExact;
  self.lynxView.layoutHeightMode = LynxViewSizeModeExact;
  [self.view addSubview:self.lynxView];
  [self.lynxView loadTemplateFromURL:@"main.lynx" initData:nil];
}
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
  self.lynxView.frame = self.view.bounds;
  self.lynxView.preferredLayoutWidth = self.view.bounds.size.width;
  self.lynxView.preferredLayoutHeight = self.view.bounds.size.height;
  [self.lynxView triggerLayout];
}
@end

@interface LaneDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end
@implementation LaneDelegate
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
  [LynxEnv sharedInstance];
  self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
  self.window.rootViewController = [LaneController new];
  [self.window makeKeyAndVisible];
  NSLog(@"{\"event\":\"lane_started\",\"platform\":\"ios\"}");
  return YES;
}
- (void)applicationDidEnterBackground:(UIApplication *)application {
  LaneController *controller = (LaneController *)self.window.rootViewController;
  [controller.lynxView onEnterBackground];
}
- (void)applicationWillEnterForeground:(UIApplication *)application {
  LaneController *controller = (LaneController *)self.window.rootViewController;
  [controller.lynxView onEnterForeground];
}
@end
int main(int argc, char *argv[]) {
  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass(LaneDelegate.class)); }
}
