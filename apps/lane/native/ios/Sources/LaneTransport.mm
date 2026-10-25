// SPDX-License-Identifier: Apache-2.0
#import "LaneTransport.h"

NSDictionary *LaneJSON(NSString *text) {
  if (![text isKindOfClass:NSString.class]) return nil;
  id value = [NSJSONSerialization JSONObjectWithData:[text dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
  return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
NSString *LaneEncode(id value) {
  NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
  return data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : @"{}";
}
NSString *LaneFailure(NSString *reason, BOOL retryable) {
  if (![reason hasPrefix:@"ERROR_REASON_"]) reason = [@"ERROR_REASON_" stringByAppendingString:reason];
  return LaneEncode(@{@"ok": @NO, @"error": @{@"reason": reason,
    @"message": @"The operation could not be completed. Retry or check Lane settings.",
    @"correlationId": NSUUID.UUID.UUIDString.lowercaseString, @"retryable": @(retryable)}});
}
@implementation LaneTransport {
  NSURLSession *_session;
  NSMutableSet<NSURLSessionTask *> *_tasks;
  BOOL _cancelled;
}
- (instancetype)init {
  if ((self = [super init])) {
    NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    config.timeoutIntervalForRequest = 20;
    config.timeoutIntervalForResource = 25;
    config.URLCache = nil;
    config.HTTPCookieStorage = nil;
    _session = [NSURLSession sessionWithConfiguration:config delegate:self delegateQueue:nil];
    _tasks = [NSMutableSet new];
  }
  return self;
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response newRequest:(NSURLRequest *)request
    completionHandler:(void (^)(NSURLRequest *))completion {
  // Do not forward bearer tokens or authorization codes through redirects.
  completion(nil);
}
- (NSDictionary *)perform:(NSMutableURLRequest *)request {
  if (![request.URL.scheme.lowercaseString isEqualToString:@"https"] || !request.URL.host.length || request.URL.user || request.URL.password)
    return LaneJSON(LaneFailure(@"INVALID_INPUT", NO));
  dispatch_semaphore_t done = dispatch_semaphore_create(0);
  __block NSData *body;
  __block NSInteger status = 0;
  __block NSError *failure;
  NSURLSessionDataTask *task = [_session dataTaskWithRequest:request completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
    body = data; status = [(NSHTTPURLResponse *)response statusCode]; failure = error;
    dispatch_semaphore_signal(done);
  }];
  @synchronized (self) {
    if (_cancelled) { [task cancel]; return LaneJSON(LaneFailure(@"ONLINE_REQUIRED", YES)); }
    [_tasks addObject:task]; [task resume];
  }
  BOOL timedOut = dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 26 * NSEC_PER_SEC)) != 0;
  if (timedOut) {
    [task cancel];
    // The completion block owns its captured data even after this method returns.
    @synchronized (self) { [_tasks removeObject:task]; }
    return LaneJSON(LaneFailure(@"ONLINE_REQUIRED", YES));
  }
  @synchronized (self) { [_tasks removeObject:task]; }
  if (failure) return LaneJSON(LaneFailure(@"ONLINE_REQUIRED", YES));
  if (status < 200 || status >= 300) {
    NSString *reason = status == 401 ? @"UNAUTHENTICATED" : status == 403 ? @"FORBIDDEN" :
      status == 429 ? @"RATE_LIMITED" : status == 413 ? @"ITEM_TOO_LARGE" : @"DEPENDENCY_UNAVAILABLE";
    return LaneJSON(LaneFailure(reason, status >= 500 || status == 429));
  }
  NSDictionary *json = LaneJSON([[NSString alloc] initWithData:body encoding:NSUTF8StringEncoding]);
  return json ?: LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES));
}
- (NSDictionary *)callService:(NSString *)service method:(NSString *)method request:(NSDictionary *)request token:(NSString *)token {
  NSString *base = [NSBundle.mainBundle objectForInfoDictionaryKey:@"LaneAPIURL"];
  if (!base.length || [base containsString:@"$("] || !token.length) return LaneJSON(LaneFailure(@"UNAUTHENTICATED", NO));
  NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"%@/%@/%@", [base stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]], service, method]];
  if (!url) return LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", NO));
  NSMutableURLRequest *http = [NSMutableURLRequest requestWithURL:url];
  http.HTTPMethod = @"POST";
  [http setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
  [http setValue:@"1" forHTTPHeaderField:@"Connect-Protocol-Version"];
  [http setValue:[@"Bearer " stringByAppendingString:token] forHTTPHeaderField:@"Authorization"];
  http.HTTPBody = [LaneEncode(request) dataUsingEncoding:NSUTF8StringEncoding];
  return [self perform:http];
}
- (NSDictionary *)getJSON:(NSURL *)url { return url ? [self perform:[NSMutableURLRequest requestWithURL:url]] : LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", NO)); }
- (NSDictionary *)postForm:(NSDictionary *)form url:(NSURL *)url {
  NSCharacterSet *safe = [NSCharacterSet characterSetWithCharactersInString:@"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"];
  NSMutableArray *parts = [NSMutableArray new];
  for (NSString *key in form) [parts addObject:[NSString stringWithFormat:@"%@=%@", [key stringByAddingPercentEncodingWithAllowedCharacters:safe], [form[key] stringByAddingPercentEncodingWithAllowedCharacters:safe]]];
  if (!url) return LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", NO));
  NSMutableURLRequest *http = [NSMutableURLRequest requestWithURL:url];
  http.HTTPMethod = @"POST";
  [http setValue:@"application/x-www-form-urlencoded" forHTTPHeaderField:@"Content-Type"];
  http.HTTPBody = [[parts componentsJoinedByString:@"&"] dataUsingEncoding:NSUTF8StringEncoding];
  return [self perform:http];
}
- (void)cancel { @synchronized (self) { _cancelled = YES; for (NSURLSessionTask *task in _tasks) [task cancel]; } }
- (void)resume { @synchronized (self) { _cancelled = NO; } }
@end
