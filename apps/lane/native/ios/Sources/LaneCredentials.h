// SPDX-License-Identifier: Apache-2.0
#import "LaneTransport.h"
NS_ASSUME_NONNULL_BEGIN
@interface LaneCredentials : NSObject
- (instancetype)initWithTransport:(LaneTransport *)transport;
- (NSDictionary *)begin:(NSString *)account device:(NSString *)device;
// Returns validated identity and credentials only to native code, never to JS.
- (nullable NSDictionary *)complete:(NSURL *)url;
- (BOOL)save:(NSDictionary *)credential space:(NSString *)space;
- (nullable NSString *)tokenForSpace:(NSString *)space;
- (void)disconnect:(NSString *)space;
@end
NSString *LaneUUID7(void);
NS_ASSUME_NONNULL_END
