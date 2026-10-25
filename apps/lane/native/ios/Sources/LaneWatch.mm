// SPDX-License-Identifier: Apache-2.0
#import "LaneWatch.h"
#import "LaneTransport.h"

@implementation LaneWatch {
  NSURLSession *_session;
  NSURLSessionDataTask *_task;
  NSMutableData *_buffer;
  dispatch_block_t _changed;
  dispatch_block_t _ended;
  BOOL _finished;
}
- (instancetype)initWithToken:(NSString *)token epoch:(NSString *)epoch cursor:(NSString *)cursor
                      changed:(dispatch_block_t)changed ended:(dispatch_block_t)ended {
  if ((self = [super init])) {
    _changed = [changed copy]; _ended = [ended copy]; _buffer = [NSMutableData new];
    NSString *base = [NSBundle.mainBundle objectForInfoDictionaryKey:@"LaneAPIURL"];
    NSURL *url = [NSURL URLWithString:[[base stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]] stringByAppendingString:@"/lane.v1.SyncService/WatchChanges"]];
    if (![url.scheme isEqualToString:@"https"] || !url.host.length || url.user || url.password || !token.length) return nil;
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url];
    request.HTTPMethod = @"POST";
    [request setValue:@"application/connect+json" forHTTPHeaderField:@"Content-Type"];
    [request setValue:@"1" forHTTPHeaderField:@"Connect-Protocol-Version"];
    [request setValue:[@"Bearer " stringByAppendingString:token] forHTTPHeaderField:@"Authorization"];
    NSData *json = [LaneEncode(@{@"epoch": epoch, @"cursor": cursor ?: @""}) dataUsingEncoding:NSUTF8StringEncoding];
    uint32_t length = CFSwapInt32HostToBig((uint32_t)json.length);
    unsigned char flag = 0;
    NSMutableData *body = [NSMutableData dataWithBytes:&flag length:1];
    [body appendBytes:&length length:4]; [body appendData:json]; request.HTTPBody = body;
    NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    config.timeoutIntervalForRequest = 65; config.timeoutIntervalForResource = 300;
    config.URLCache = nil; config.HTTPCookieStorage = nil;
    NSOperationQueue *queue = [NSOperationQueue new]; queue.maxConcurrentOperationCount = 1;
    _session = [NSURLSession sessionWithConfiguration:config delegate:self delegateQueue:queue];
    _task = [_session dataTaskWithRequest:request]; [_task resume];
  } return self;
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task willPerformHTTPRedirection:(NSHTTPURLResponse *)response
        newRequest:(NSURLRequest *)request completionHandler:(void (^)(NSURLRequest *))completion { completion(nil); }
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task didReceiveResponse:(NSURLResponse *)response
 completionHandler:(void (^)(NSURLSessionResponseDisposition))completion {
  BOOL valid = [(NSHTTPURLResponse *)response statusCode] == 200 && [response.MIMEType isEqualToString:@"application/connect+json"];
  completion(valid ? NSURLSessionResponseAllow : NSURLSessionResponseCancel);
}
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task didReceiveData:(NSData *)data {
  [_buffer appendData:data];
  while (_buffer.length >= 5) {
    const unsigned char *bytes = (const unsigned char *)_buffer.bytes;
    uint32_t length = ((uint32_t)bytes[1] << 24) | ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 8) | bytes[4];
    // A WatchChanges frame contains one cursor, not task content. Bound malformed frames.
    if (length > 1024 * 1024 || (bytes[0] != 0 && bytes[0] != 2)) { [_task cancel]; return; }
    if (_buffer.length < (NSUInteger)length + 5) return;
    BOOL end = bytes[0] == 2;
    NSData *body = [_buffer subdataWithRange:NSMakeRange(5, length)];
    NSDictionary *message = LaneJSON([[NSString alloc] initWithData:body encoding:NSUTF8StringEncoding]);
    [_buffer replaceBytesInRange:NSMakeRange(0, (NSUInteger)length + 5) withBytes:NULL length:0];
    if (!message || end) { [_task cancel]; return; }
    if ([message[@"cursor"] isKindOfClass:NSString.class]) _changed();
  }
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
  @synchronized (self) {
    if (!_finished) { _finished = YES; _ended(); }
  }
  [_session finishTasksAndInvalidate];
}
- (void)cancel {
  @synchronized (self) { _finished = YES; }
  [_session invalidateAndCancel];
}
@end
