// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
// Native-only, one-use proof storage. Caller serializes access on the store queue.
@interface LaneReauthentication : NSObject
- (void)beginSpace:(NSString *)space account:(NSString *)account mode:(NSString *)mode challenge:(NSString *)challenge;
- (nullable NSDictionary *)bindingForChallenge:(NSString *)challenge selectedSpace:(NSString *)space;
- (BOOL)acceptProof:(NSString *)proof challenge:(NSString *)challenge selectedSpace:(NSString *)space;
- (nullable NSString *)consumeForSpace:(NSString *)space account:(NSString *)account mode:(NSString *)mode challenge:(NSString *)challenge;
- (void)cancel:(nullable NSString *)challenge;
@end
NS_ASSUME_NONNULL_END
