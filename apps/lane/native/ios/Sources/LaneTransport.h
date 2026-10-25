// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
// Synchronous requests run only on the host's storage worker. Cancellation is thread-safe.
@interface LaneTransport : NSObject <NSURLSessionTaskDelegate>
- (NSDictionary *)callService:(NSString *)service method:(NSString *)method
                     request:(NSDictionary *)request token:(NSString *)token;
- (NSDictionary *)postForm:(NSDictionary *)form url:(NSURL *)url;
- (NSDictionary *)getJSON:(NSURL *)url;
- (void)cancel;
- (void)resume;
@end
NSDictionary *LaneJSON(NSString *text);
NSString *LaneEncode(id value);
NSString *LaneFailure(NSString *reason, BOOL retryable);
NS_ASSUME_NONNULL_END
