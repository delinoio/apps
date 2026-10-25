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
// Decode the canonical lane.v1.ErrorDetail from Connect's base64 protobuf detail.
// Keep server content/messages out of local diagnostic errors.
static BOOL ReadVarint(const uint8_t *bytes, NSUInteger size, NSUInteger *offset, uint64_t *value) {
  *value = 0;
  for (NSUInteger shift = 0; shift < 64 && *offset < size; shift += 7) {
    uint8_t byte = bytes[(*offset)++];
    if (shift == 63 && byte > 1) return NO;
    *value |= (uint64_t)(byte & 127) << shift;
    if (!(byte & 128)) return YES;
  }
  return NO;
}
NSDictionary *LaneConnectFailure(NSData *body, NSInteger status) {
  NSArray *reasons = @[@"UNSPECIFIED", @"INVALID_INPUT", @"NOT_FOUND", @"STALE_VERSION", @"CONFLICT_READ_ONLY", @"STALE_RESOLUTION", @"REQUEST_ID_REUSED", @"DELETED", @"UNSUPPORTED_FORMAT", @"STORAGE_BUSY", @"STORAGE_FULL", @"ONLINE_REQUIRED", @"UNAUTHENTICATED", @"FORBIDDEN", @"DEPENDENCY_UNAVAILABLE", @"ITEM_TOO_LARGE", @"RATE_LIMITED", @"STALE_EPOCH", @"LEASE_INVALID"];
  NSDictionary *json = LaneJSON([[NSString alloc] initWithData:body encoding:NSUTF8StringEncoding]);
  NSString *reason = status == 401 ? @"UNAUTHENTICATED" : status == 403 ? @"FORBIDDEN" : status == 429 ? @"RATE_LIMITED" : status == 413 ? @"ITEM_TOO_LARGE" : @"DEPENDENCY_UNAVAILABLE";
  NSMutableDictionary *error = [LaneJSON(LaneFailure(reason, status >= 500 || status == 429))[@"error"] mutableCopy];
  if (![json[@"details"] isKindOfClass:NSArray.class]) return @{@"ok": @NO, @"error": error};
  for (id item in json[@"details"]) {
    if (![item isKindOfClass:NSDictionary.class] || ![item[@"type"] isEqual:@"lane.v1.ErrorDetail"] || ![item[@"value"] isKindOfClass:NSString.class]) continue;
    NSData *data = [[NSData alloc] initWithBase64EncodedString:item[@"value"] options:0];
    const uint8_t *bytes = (const uint8_t *)data.bytes; NSUInteger offset = 0;
    NSMutableDictionary *decoded = [error mutableCopy]; NSMutableArray *heads = [NSMutableArray new]; BOOL valid = YES;
    while (offset < data.length) {
      uint64_t tag, value;
      if (!ReadVarint(bytes, data.length, &offset, &tag) || !(tag >> 3)) { valid = NO; break; }
      NSUInteger field = tag >> 3, wire = tag & 7;
      if (wire == 0) {
        if (!ReadVarint(bytes, data.length, &offset, &value)) { valid = NO; break; }
        if (field == 1 && value > 0 && value < reasons.count) decoded[@"reason"] = [@"ERROR_REASON_" stringByAppendingString:reasons[value]];
        if (field == 5) decoded[@"retryable"] = @(value != 0);
      } else if (wire == 2) {
        if (!ReadVarint(bytes, data.length, &offset, &value) || value > data.length - offset) { valid = NO; break; }
        NSString *text = [[NSString alloc] initWithBytes:bytes + offset length:(NSUInteger)value encoding:NSUTF8StringEncoding];
        if (text && field == 2) decoded[@"correlationId"] = text;
        if (text && field == 3) decoded[@"entityId"] = text;
        if (text && field == 4) [heads addObject:text];
        offset += (NSUInteger)value;
      } else if (wire == 1 || wire == 5) {
        NSUInteger width = wire == 1 ? 8 : 4;
        if (width > data.length - offset) { valid = NO; break; } offset += width;
      } else { valid = NO; break; }
    }
    if (valid && data.length) { if (heads.count) decoded[@"currentHeadIds"] = heads; error = decoded; break; }
  }
  return @{@"ok": @NO, @"error": error};
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
    return LaneConnectFailure(body, status);
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
