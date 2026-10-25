// SPDX-License-Identifier: Apache-2.0
#import "LaneCredentials.h"
#import <Security/Security.h>
#import <CommonCrypto/CommonDigest.h>

static NSString *Text(id value) { return [value isKindOfClass:NSString.class] ? value : @""; }
static double Number(id value) { return [value isKindOfClass:NSNumber.class] || [value isKindOfClass:NSString.class] ? [value doubleValue] : 0; }
static NSString *Base64URL(NSData *data) {
  return [[[[data base64EncodedStringWithOptions:0] stringByReplacingOccurrencesOfString:@"+" withString:@"-"] stringByReplacingOccurrencesOfString:@"/" withString:@"_"] stringByReplacingOccurrencesOfString:@"=" withString:@""];
}
NSString *LaneUUID7(void) {
  unsigned char b[16];
  if (SecRandomCopyBytes(kSecRandomDefault, sizeof(b), b) != errSecSuccess) return nil;
  uint64_t ms = (uint64_t)(NSDate.date.timeIntervalSince1970 * 1000);
  for (int i = 5; i >= 0; --i) { b[i] = ms & 255; ms >>= 8; }
  b[6] = (b[6] & 15) | 0x70; b[8] = (b[8] & 63) | 0x80;
  return [[NSUUID alloc] initWithUUIDBytes:b].UUIDString.lowercaseString;
}
@implementation LaneCredentials {
  LaneTransport *_transport;
  NSDictionary *_pending;
  NSDictionary *_configuration;
}
- (instancetype)initWithTransport:(LaneTransport *)transport {
  return [self initWithTransport:transport configuration:NSBundle.mainBundle.infoDictionary];
}
- (instancetype)initWithTransport:(LaneTransport *)transport configuration:(NSDictionary *)configuration {
  if ((self = [super init])) { _transport = transport; _configuration = [configuration copy]; }
  return self;
}
- (NSURL *)endpoint:(NSString *)path {
  NSString *base = _configuration[@"LaneAPIURL"];
  NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"%@%@", [base stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]], path]];
  return [url.scheme isEqualToString:@"https"] && url.host.length && !url.user && !url.password ? url : nil;
}
- (NSString *)client { return _configuration[@"LaneOAuthClientID"]; }
- (NSString *)redirect {
  NSString *scheme = _configuration[@"LaneURLScheme"];
  return [NSString stringWithFormat:@"%@://auth/callback", scheme];
}
- (NSDictionary *)begin:(NSString *)account device:(NSString *)device {
  @synchronized (self) {
  NSURL *endpoint = [self endpoint:@"/oauth/authorize"];
  if (!endpoint || ![self client].length || !device.length) return LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", NO));
  unsigned char verifier[32], state[32];
  if (SecRandomCopyBytes(kSecRandomDefault, sizeof(verifier), verifier) || SecRandomCopyBytes(kSecRandomDefault, sizeof(state), state))
    return LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES));
  NSString *verifierText = Base64URL([NSData dataWithBytes:verifier length:sizeof(verifier)]);
  NSString *stateText = Base64URL([NSData dataWithBytes:state length:sizeof(state)]);
  NSData *input = [verifierText dataUsingEncoding:NSUTF8StringEncoding];
  unsigned char digest[CC_SHA256_DIGEST_LENGTH]; CC_SHA256(input.bytes, (CC_LONG)input.length, digest);
  NSURLComponents *url = [NSURLComponents componentsWithURL:endpoint resolvingAgainstBaseURL:NO];
  url.queryItems = @[[NSURLQueryItem queryItemWithName:@"response_type" value:@"code"],
    [NSURLQueryItem queryItemWithName:@"client_id" value:[self client]],
    [NSURLQueryItem queryItemWithName:@"redirect_uri" value:[self redirect]],
    [NSURLQueryItem queryItemWithName:@"state" value:stateText],
    [NSURLQueryItem queryItemWithName:@"code_challenge" value:Base64URL([NSData dataWithBytes:digest length:sizeof(digest)])],
    [NSURLQueryItem queryItemWithName:@"code_challenge_method" value:@"S256"],
    [NSURLQueryItem queryItemWithName:@"device_name" value:@"Lane for iOS"],
    [NSURLQueryItem queryItemWithName:@"device_id" value:device]];
  _pending = @{@"state": stateText, @"verifier": verifierText, @"account": account ?: @"", @"device": device,
               @"expires": @(NSDate.date.timeIntervalSince1970 + 600)};
  return @{@"authorizationUrl": url.URL.absoluteString, @"challengeId": stateText};
}
}
- (NSDictionary *)complete:(NSURL *)url {
  @synchronized (self) {
  NSURLComponents *parts = [NSURLComponents componentsWithURL:url resolvingAgainstBaseURL:NO];
  if (![[NSString stringWithFormat:@"%@://%@%@", parts.scheme, parts.host, parts.path] isEqualToString:[self redirect]] || parts.user || parts.password || parts.port || parts.fragment)
    return nil;
  NSMutableDictionary *query = [NSMutableDictionary new];
  for (NSURLQueryItem *item in parts.queryItems) {
    if (query[item.name] || !item.value) return nil;
    query[item.name] = item.value;
  }
  if (!_pending || ![query[@"state"] isEqual:_pending[@"state"]] || [_pending[@"expires"] doubleValue] < NSDate.date.timeIntervalSince1970) return nil;
  NSDictionary *pending = _pending; _pending = nil; // Consume even failed/canceled callbacks.
  if (![query[@"code"] length] || query[@"error"]) return nil;
  NSDictionary *token = [_transport postForm:@{@"grant_type": @"authorization_code", @"code": query[@"code"],
    @"client_id": [self client], @"redirect_uri": [self redirect], @"code_verifier": pending[@"verifier"]}
    url:[self endpoint:@"/oauth/token"]];
  if (![token[@"device_id"] isEqual:pending[@"device"]] || ![token[@"access_token"] isKindOfClass:NSString.class] || [Text(token[@"token_type"]) caseInsensitiveCompare:@"Bearer"] != NSOrderedSame || Number(token[@"expires_in"]) <= 0) return nil;
  NSDictionary *info = [_transport callService:@"lane.v1.SyncService" method:@"GetSyncInfo" request:@{} token:token[@"access_token"]];
  if (Number(info[@"formatVersion"]) != 1 || !Text(info[@"accountId"]).length || !Text(info[@"tenantId"]).length || !Text(info[@"epoch"]).length ||
      ![info[@"accountId"] isEqual:token[@"account_id"]] || ![info[@"tenantId"] isEqual:token[@"tenant_id"]] ||
      ([pending[@"account"] length] && ![pending[@"account"] isEqual:info[@"accountId"]])) return nil;
  NSURL *issuer = [NSURL URLWithString:Text(token[@"issuer"])];
  if (![issuer.scheme isEqualToString:@"https"] || !issuer.host.length) return nil;
  NSMutableDictionary *credential = [token mutableCopy];
  credential[@"expiresAt"] = @(NSDate.date.timeIntervalSince1970 + Number(token[@"expires_in"]));
  credential[@"approvalState"] = pending[@"state"];
  credential[@"identity"] = @{@"issuer": token[@"issuer"], @"accountId": info[@"accountId"], @"tenantId": info[@"tenantId"], @"epoch": info[@"epoch"], @"deviceId": token[@"device_id"]};
  return credential;
}
}
- (NSMutableDictionary *)key:(NSString *)space {
  return [@{(__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
    (__bridge id)kSecAttrService: @"io.delino.lane.sync", (__bridge id)kSecAttrAccount: space} mutableCopy];
}
- (NSDictionary *)read:(NSString *)space {
  NSMutableDictionary *query = [self key:space]; query[(__bridge id)kSecReturnData] = @YES;
  CFTypeRef result = nil;
  if (SecItemCopyMatching((__bridge CFDictionaryRef)query, &result) != errSecSuccess) return nil;
  NSData *data = CFBridgingRelease(result);
  return LaneJSON([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]);
}
- (BOOL)save:(NSDictionary *)credential space:(NSString *)space {
  @synchronized (self) {
  NSMutableDictionary *query = [self key:space];
  NSDictionary *attributes = @{(__bridge id)kSecValueData: [LaneEncode(credential) dataUsingEncoding:NSUTF8StringEncoding],
    (__bridge id)kSecAttrAccessible: (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly};
  OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)attributes);
  if (status == errSecItemNotFound) { [query addEntriesFromDictionary:attributes]; status = SecItemAdd((__bridge CFDictionaryRef)query, nil); }
  return status == errSecSuccess;
}
}
- (NSString *)lastAccessTokenForSpace:(NSString *)space {
  @synchronized (self) { return [self read:space][@"access_token"]; }
}
- (NSString *)deviceForSpace:(NSString *)space {
  @synchronized (self) { return [self read:space][@"device_id"]; }
}
- (NSString *)tokenForSpace:(NSString *)space {
  @synchronized (self) {
  NSDictionary *credential = [self read:space];
  if (!credential) return nil;
  if (Number(credential[@"expiresAt"]) > NSDate.date.timeIntervalSince1970 + 60) return credential[@"access_token"];
  if (!Text(credential[@"refresh_token"]).length) return nil;
  NSDictionary *fresh = [_transport postForm:@{@"grant_type": @"refresh_token", @"refresh_token": credential[@"refresh_token"], @"client_id": [self client]} url:[self endpoint:@"/oauth/token"]];
  if (!Text(fresh[@"access_token"]).length || [Text(fresh[@"token_type"]) caseInsensitiveCompare:@"Bearer"] != NSOrderedSame || Number(fresh[@"expires_in"]) <= 0) return nil;
  // A refresh must not change the account bound to this local space.
  for (NSString *key in @[@"issuer", @"account_id", @"tenant_id", @"device_id", @"session_id"])
    if (fresh[key] && ![fresh[key] isEqual:credential[key]]) return nil;
  NSMutableDictionary *updated = [credential mutableCopy]; [updated addEntriesFromDictionary:fresh];
  updated[@"expiresAt"] = @(NSDate.date.timeIntervalSince1970 + Number(fresh[@"expires_in"]));
  return [self save:updated space:space] ? updated[@"access_token"] : nil;
}
}
- (void)forget:(NSString *)space { @synchronized (self) { SecItemDelete((__bridge CFDictionaryRef)[self key:space]); } }
- (void)disconnect:(NSString *)space {
  @synchronized (self) {
  NSDictionary *credential = [self read:space];
  SecItemDelete((__bridge CFDictionaryRef)[self key:space]);
  NSString *token = credential[@"refresh_token"] ?: credential[@"access_token"];
  if (token.length) [_transport postForm:@{@"token": token, @"client_id": [self client]} url:[self endpoint:@"/oauth/revoke"]];
}
}
@end
