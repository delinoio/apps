// SPDX-License-Identifier: Apache-2.0
#import "LaneHost.h"

@implementation LaneStoreModule {
  NSMutableSet<NSString *> *_spaces;
  BOOL _destroyed;
}
+ (NSString *)name { return @"LaneStore"; }
+ (NSDictionary<NSString *, NSString *> *)methodLookup {
  return @{@"open": NSStringFromSelector(@selector(open:callback:)),
           @"invoke": NSStringFromSelector(@selector(invoke:request:callback:)),
           @"close": NSStringFromSelector(@selector(close:callback:))};
}
- (instancetype)init {
  if ((self = [super init])) _spaces = [NSMutableSet new];
  return self;
}
- (void)open:(NSString *)json callback:(LynxCallbackBlock)callback {
  [[LaneHost shared] open:json completion:^(id result) {
    NSDictionary *value = [NSJSONSerialization JSONObjectWithData:[result dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    NSString *space = value[@"response"][@"spaceId"] ?: value[@"spaceId"];
    @synchronized (self) {
      if (space && self->_destroyed) {
        [[LaneHost shared] close:space completion:^(id ignored) {}];
        return;
      }
      if (space) [self->_spaces addObject:space];
      if (!self->_destroyed) callback(result);
    }
  }];
}
- (void)invoke:(NSString *)method request:(NSString *)json callback:(LynxCallbackBlock)callback {
  [[LaneHost shared] invoke:method json:json completion:^(id result) {
    @synchronized (self) { if (!self->_destroyed) callback(result); }
  }];
}
- (void)close:(NSString *)space callback:(LynxCallbackBlock)callback {
  @synchronized (self) { [_spaces removeObject:space]; }
  [[LaneHost shared] close:space completion:callback];
}
- (void)destroy {
  @synchronized (self) {
    _destroyed = YES;
    for (NSString *space in _spaces) [[LaneHost shared] close:space completion:^(id ignored) {}];
    [_spaces removeAllObjects];
  }
}
@end
