// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

typedef void (^LaneHTTPSImageDataCompletion)(NSData *_Nullable data, NSError *_Nullable error);

@interface LaneHTTPSImageData : NSObject
- (instancetype)initWithConfiguration:(NSURLSessionConfiguration *)configuration;
- (dispatch_block_t)loadURL:(NSURL *)url completion:(LaneHTTPSImageDataCompletion)completion;
+ (BOOL)allowsURL:(NSURL *)url;
+ (BOOL)isPNG:(NSData *)data;
@end

NS_ASSUME_NONNULL_END
