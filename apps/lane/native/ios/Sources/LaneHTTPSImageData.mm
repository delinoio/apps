// SPDX-License-Identifier: Apache-2.0
#import "LaneHTTPSImageData.h"

static NSString *const LaneImageErrorDomain = @"io.delino.lane.image";
static NSUInteger const LaneImageMaxBytes = 8 * 1024 * 1024;

@interface LaneHTTPSResponseGuard : NSObject <NSURLSessionDataDelegate>
@property(nonatomic, strong) NSProgress *progress;
@property(nonatomic, copy) LaneHTTPSImageDataCompletion completion;
@property(nonatomic, strong) NSMutableData *buffer;
@property(nonatomic) BOOL accepted;
- (instancetype)initWithProgress:(NSProgress *)progress completion:(LaneHTTPSImageDataCompletion)completion;
@end
@implementation LaneHTTPSResponseGuard
- (instancetype)initWithProgress:(NSProgress *)progress completion:(LaneHTTPSImageDataCompletion)completion {
  if ((self = [super init])) {
    _progress = progress;
    _completion = [completion copy];
    _buffer = [NSMutableData data];
  }
  return self;
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response newRequest:(NSURLRequest *)request
    completionHandler:(void (^)(NSURLRequest *_Nullable))completionHandler {
  // NSURLSession follows redirects by default. Reject a downgrade before it sends a request.
  completionHandler([LaneHTTPSImageData allowsURL:request.URL] ? request : nil);
}
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)dataTask
    didReceiveResponse:(NSURLResponse *)response
    completionHandler:(void (^)(NSURLSessionResponseDisposition))completionHandler {
  BOOL valid = [response isKindOfClass:NSHTTPURLResponse.class] &&
      ((NSHTTPURLResponse *)response).statusCode == 200 &&
      (response.expectedContentLength < 0 || response.expectedContentLength <= (long long)LaneImageMaxBytes);
  self.accepted = valid;
  completionHandler(valid ? NSURLSessionResponseAllow : NSURLSessionResponseCancel);
}
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)dataTask didReceiveData:(NSData *)data {
  if (!self.accepted) return;
  if (data.length > LaneImageMaxBytes - self.buffer.length) {
    self.accepted = NO;
    [dataTask cancel];
    return;
  }
  [self.buffer appendData:data];
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
  BOOL valid = !error && self.accepted && [LaneHTTPSImageData isPNG:self.buffer];
  // NSURLSession errors can include the source URL, so report a fixed host error.
  if (!self.progress.cancelled)
    self.completion(valid ? [self.buffer copy] : nil,
        valid ? nil : [NSError errorWithDomain:LaneImageErrorDomain code:1 userInfo:nil]);
  [session finishTasksAndInvalidate];
}
@end

@interface LaneHTTPSImageData ()
@property(nonatomic, strong) NSURLSessionConfiguration *configuration;
@end
@implementation LaneHTTPSImageData
- (instancetype)init { return [self initWithConfiguration:NSURLSessionConfiguration.ephemeralSessionConfiguration]; }
- (instancetype)initWithConfiguration:(NSURLSessionConfiguration *)configuration {
  if ((self = [super init])) {
    _configuration = [configuration copy];
    _configuration.URLCache = nil;
    _configuration.HTTPCookieStorage = nil;
    _configuration.HTTPCookieAcceptPolicy = NSHTTPCookieAcceptPolicyNever;
    _configuration.requestCachePolicy = NSURLRequestReloadIgnoringLocalCacheData;
    _configuration.timeoutIntervalForRequest = 15;
    _configuration.timeoutIntervalForResource = 30;
  }
  return self;
}
+ (BOOL)allowsURL:(NSURL *)url {
  return [url.scheme.lowercaseString isEqualToString:@"https"] && url.host.length > 0 &&
      url.user == nil && url.password == nil;
}
+ (BOOL)isPNG:(NSData *)data {
  static const unsigned char signature[] = {0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a};
  return data.length >= sizeof(signature) && memcmp(data.bytes, signature, sizeof(signature)) == 0;
}
- (dispatch_block_t)loadURL:(NSURL *)url completion:(LaneHTTPSImageDataCompletion)completion {
  NSProgress *progress = [NSProgress progressWithTotalUnitCount:1];
  if (![LaneHTTPSImageData allowsURL:url]) {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
      if (!progress.cancelled) completion(nil, [NSError errorWithDomain:LaneImageErrorDomain code:1 userInfo:nil]);
    });
    return ^{ [progress cancel]; };
  }
  LaneHTTPSResponseGuard *guard = [[LaneHTTPSResponseGuard alloc] initWithProgress:progress completion:completion];
  NSURLSession *session = [NSURLSession sessionWithConfiguration:self.configuration delegate:guard delegateQueue:nil];
  NSURLRequest *request = [NSURLRequest requestWithURL:url cachePolicy:NSURLRequestReloadIgnoringLocalCacheData
      timeoutInterval:15];
  NSURLSessionDataTask *task = [session dataTaskWithRequest:request];
  [task resume];
  return ^{
    [progress cancel];
    [task cancel];
    [session invalidateAndCancel];
  };
}
@end
