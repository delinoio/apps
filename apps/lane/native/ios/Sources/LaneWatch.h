// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
// Connect streaming invalidations carry cursors, never authoritative entity data.
@interface LaneWatch : NSObject <NSURLSessionDataDelegate>
- (instancetype)initWithToken:(NSString *)token epoch:(NSString *)epoch cursor:(NSString *)cursor
                      changed:(dispatch_block_t)changed ended:(dispatch_block_t)ended;
- (void)cancel;
@end
NS_ASSUME_NONNULL_END
