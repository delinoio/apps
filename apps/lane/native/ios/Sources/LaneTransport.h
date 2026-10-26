// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
// Synchronous requests run only on the host's storage worker. Cancellation is thread-safe.
@interface LaneTransport : NSObject <NSURLSessionTaskDelegate>
- (NSDictionary *)callService:(NSString *)service method:(NSString *)method
                     request:(NSDictionary *)request token:(NSString *)token;
- (NSDictionary *)postForm:(NSDictionary *)form url:(NSURL *)url;
- (NSDictionary *)attachment:(NSURL *)url;
- (NSDictionary *)getJSON:(NSURL *)url;
- (NSDictionary *)postJSON:(NSDictionary *)json path:(NSString *)path token:(NSString *)token;
- (NSDictionary *)getPath:(NSString *)path query:(NSDictionary *)query token:(NSString *)token;
- (void)cancel;
- (void)resume;
@end
NSDictionary *LaneJSON(NSString *text);
NSString *LaneEncode(id value);
NSString *LaneFailure(NSString *reason, BOOL retryable);
NSDictionary *LaneConnectFailure(NSData *body, NSInteger status);
NS_ASSUME_NONNULL_END
