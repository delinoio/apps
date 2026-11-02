// SPDX-License-Identifier: Apache-2.0
#import "AnyDashHost.h"
#import <AuthenticationServices/AuthenticationServices.h>
#import <CommonCrypto/CommonDigest.h>
#import <Security/Security.h>
#import <UIKit/UIKit.h>
#import "AnyDash-Swift.h"

static NSString *const kService = @"io.delino.anydash.session";
static NSString *const kRefreshAccount = @"refresh-token";
static NSString *const kUserKeyAccount = @"user-key";
static NSString *const kWidgetAccount = @"widget-access-token";
static NSString *const kAppGroup = @"group.io.delino.anydash";
static NSString *const kRedirect = @"anydash://auth/callback";

static void AnyDashLog(NSString *event, NSDictionary *fields) {
  NSMutableDictionary *entry = [@{@"event": event, @"platform": @"ios"} mutableCopy];
  [entry addEntriesFromDictionary:fields ?: @{}];
  NSData *data = [NSJSONSerialization dataWithJSONObject:entry options:0 error:nil];
  NSLog(@"%@", [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]);
}

/// Appends `delcore_access_token` (field 1, length-delimited) to a binary
/// Protobuf request. A later occurrence of a singular field wins when the server
/// parses the message, so a value set by the bundle is replaced.
static NSData *WithDelcoreToken(NSData *body, NSString *token) {
  NSData *value = [token dataUsingEncoding:NSUTF8StringEncoding];
  NSMutableData *result = [body mutableCopy];
  uint8_t tag = 0x0a;
  [result appendBytes:&tag length:1];
  NSUInteger remaining = value.length;
  while (remaining > 0x7f) {
    uint8_t byte = (uint8_t)((remaining & 0x7f) | 0x80);
    [result appendBytes:&byte length:1];
    remaining >>= 7;
  }
  uint8_t last = (uint8_t)remaining;
  [result appendBytes:&last length:1];
  [result appendData:value];
  return result;
}

static NSString *Reply(id value, NSString *error) {
  NSDictionary *body = error ? @{@"ok": @NO, @"error": @{@"message": error}} : @{@"ok": @YES, @"value": value ?: @{}};
  NSData *data = [NSJSONSerialization dataWithJSONObject:body options:0 error:nil];
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

static NSString *Base64URL(NSData *data) {
  return [[[[data base64EncodedStringWithOptions:0] stringByReplacingOccurrencesOfString:@"+" withString:@"-"]
      stringByReplacingOccurrencesOfString:@"/" withString:@"_"] stringByReplacingOccurrencesOfString:@"=" withString:@""];
}
static NSData *RandomBytes(NSUInteger count) {
  NSMutableData *data = [NSMutableData dataWithLength:count];
  if (SecRandomCopyBytes(kSecRandomDefault, count, data.mutableBytes) != errSecSuccess) return nil;
  return data;
}
static NSString *SHA256Hex(NSString *text) {
  NSData *input = [text dataUsingEncoding:NSUTF8StringEncoding];
  unsigned char digest[CC_SHA256_DIGEST_LENGTH];
  CC_SHA256(input.bytes, (CC_LONG)input.length, digest);
  NSMutableString *hex = [NSMutableString string];
  for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++) [hex appendFormat:@"%02x", digest[i]];
  return hex;
}

#pragma mark - Keychain

static NSMutableDictionary *KeychainQuery(NSString *account, NSString *group) {
  NSMutableDictionary *query = [@{(__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
                                  (__bridge id)kSecAttrService: kService,
                                  (__bridge id)kSecAttrAccount: account} mutableCopy];
  if (group.length) query[(__bridge id)kSecAttrAccessGroup] = group;
  return query;
}
static NSString *KeychainGet(NSString *account, NSString *group) {
  NSMutableDictionary *query = KeychainQuery(account, group);
  query[(__bridge id)kSecReturnData] = @YES;
  CFTypeRef result = NULL;
  if (SecItemCopyMatching((__bridge CFDictionaryRef)query, &result) != errSecSuccess) return nil;
  NSData *data = (__bridge_transfer NSData *)result;
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}
static BOOL KeychainSet(NSString *account, NSString *value, NSString *group) {
  NSMutableDictionary *query = KeychainQuery(account, group);
  NSDictionary *attributes = @{(__bridge id)kSecValueData: [value dataUsingEncoding:NSUTF8StringEncoding],
                               (__bridge id)kSecAttrAccessible: (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly};
  OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)attributes);
  if (status == errSecItemNotFound) {
    [query addEntriesFromDictionary:attributes];
    status = SecItemAdd((__bridge CFDictionaryRef)query, NULL);
  }
  return status == errSecSuccess;
}
static void KeychainRemove(NSString *account, NSString *group) {
  SecItemDelete((__bridge CFDictionaryRef)KeychainQuery(account, group));
}

@interface AnyDashHost () <ASWebAuthenticationPresentationContextProviding>
@property(nonatomic, strong) NSDictionary *config;
/// Guarded by @synchronized (self), together with `sessionGeneration` and the
/// session Keychain writes that depend on the current session.
@property(nonatomic, strong) NSMutableDictionary<NSString *, NSDictionary *> *tokens;
/// Sign-in and sign-out advance it, so a token request that started under an
/// earlier session can neither store its rotated refresh token nor cache or
/// return its access token, and its failure cannot end a newer session.
@property(nonatomic) NSUInteger sessionGeneration;
@property(nonatomic, strong) ASWebAuthenticationSession *authSession;
@property(nonatomic, copy) NSString *pendingRoute;
@end

@implementation AnyDashHost
+ (instancetype)shared {
  static AnyDashHost *host;
  static dispatch_once_t once;
  dispatch_once(&once, ^{ host = [AnyDashHost new]; });
  return host;
}
- (instancetype)init {
  if ((self = [super init])) {
    NSDictionary *info = NSBundle.mainBundle.infoDictionary;
    _config = @{
      @"api": info[@"AnyDashAPIURL"] ?: @"https://anydash.api.delino.io",
      @"anydashResource": info[@"AnyDashResource"] ?: @"https://anydash.api.delino.io",
      @"delcoreResource": info[@"DelcoreResource"] ?: @"https://delcore.api.delino.io",
      @"endpoint": info[@"LogtoEndpoint"] ?: @"",
      @"appId": info[@"LogtoAppID"] ?: @"",
      @"sharedGroup": info[@"AnyDashSharedKeychainGroup"] ?: @"",
      @"version": info[@"CFBundleShortVersionString"] ?: @"0.0.0",
    };
    _tokens = [NSMutableDictionary dictionary];
  }
  return self;
}

- (ASPresentationAnchor)presentationAnchorForWebAuthenticationSession:(ASWebAuthenticationSession *)session {
  for (UIScene *scene in UIApplication.sharedApplication.connectedScenes)
    if ([scene isKindOfClass:UIWindowScene.class])
      for (UIWindow *window in ((UIWindowScene *)scene).windows)
        if (window.isKeyWindow) return window;
  return UIApplication.sharedApplication.windows.firstObject;
}

- (void)emit:(NSString *)name params:(NSDictionary *)params {
  dispatch_async(dispatch_get_main_queue(), ^{ [self.view sendGlobalEvent:name withParams:@[params]]; });
}
- (void)setVisible:(BOOL)visible { [self emit:@"anydash.visibility" params:@{@"visible": @(visible)}]; }

- (BOOL)handleURL:(NSURL *)url {
  if (![url.scheme.lowercaseString isEqualToString:@"anydash"]) return NO;
  if ([url.host isEqualToString:@"auth"]) return YES;  // ASWebAuthenticationSession owns callbacks.
  // The shared client validates every route; unknown links show "not found".
  [self emit:@"anydash.route" params:@{@"path": url.absoluteString}];
  return YES;
}

#pragma mark - Logto

- (void)tokenRequest:(NSDictionary *)params
          generation:(NSUInteger)generation
          completion:(void (^)(NSDictionary *body, NSString *error))completion {
  NSURL *url = [NSURL URLWithString:@"/oidc/token" relativeToURL:[NSURL URLWithString:self.config[@"endpoint"]]];
  NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url];
  request.HTTPMethod = @"POST";
  [request setValue:@"application/x-www-form-urlencoded" forHTTPHeaderField:@"Content-Type"];
  NSURLComponents *form = [NSURLComponents new];
  NSMutableArray *items = [NSMutableArray arrayWithObject:[NSURLQueryItem queryItemWithName:@"client_id" value:self.config[@"appId"]]];
  for (NSString *key in params) [items addObject:[NSURLQueryItem queryItemWithName:key value:params[key]]];
  form.queryItems = items;
  // Encode "+" explicitly; form decoding would read it as a space.
  request.HTTPBody = [[form.percentEncodedQuery stringByReplacingOccurrencesOfString:@"+" withString:@"%2B"] dataUsingEncoding:NSUTF8StringEncoding];
  [[NSURLSession.sharedSession dataTaskWithRequest:request completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
    NSInteger status = [(NSHTTPURLResponse *)response statusCode];
    NSDictionary *body = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    if (error || status != 200 || ![body isKindOfClass:NSDictionary.class]) {
      AnyDashLog(@"anydash_token_failed", @{@"status": @(status)});
      // An invalid grant ends the session that issued the request; transient failures keep it.
      if (status == 400) [self clearSessionIfCurrent:generation];
      completion(nil, @"Sign-in expired. Sign in again.");
      return;
    }
    completion(body, nil);
  }] resume];
}

- (void)signIn:(void (^)(NSString *error))completion {
  if (![self.config[@"endpoint"] length] || ![self.config[@"appId"] length]) {
    completion(@"Sign-in is not configured for this build.");
    return;
  }
  NSString *verifier = Base64URL(RandomBytes(32));
  NSString *state = Base64URL(RandomBytes(16));
  NSData *verifierData = [verifier dataUsingEncoding:NSUTF8StringEncoding];
  unsigned char digest[CC_SHA256_DIGEST_LENGTH];
  CC_SHA256(verifierData.bytes, (CC_LONG)verifierData.length, digest);
  NSString *challenge = Base64URL([NSData dataWithBytes:digest length:CC_SHA256_DIGEST_LENGTH]);
  NSURLComponents *url = [NSURLComponents componentsWithString:[self.config[@"endpoint"] stringByAppendingString:@"/oidc/auth"]];
  url.queryItems = @[
    [NSURLQueryItem queryItemWithName:@"client_id" value:self.config[@"appId"]],
    [NSURLQueryItem queryItemWithName:@"redirect_uri" value:kRedirect],
    [NSURLQueryItem queryItemWithName:@"response_type" value:@"code"],
    [NSURLQueryItem queryItemWithName:@"scope" value:@"openid offline_access profile"],
    [NSURLQueryItem queryItemWithName:@"code_challenge" value:challenge],
    [NSURLQueryItem queryItemWithName:@"code_challenge_method" value:@"S256"],
    [NSURLQueryItem queryItemWithName:@"state" value:state],
    [NSURLQueryItem queryItemWithName:@"prompt" value:@"consent"],
    [NSURLQueryItem queryItemWithName:@"resource" value:self.config[@"anydashResource"]],
    [NSURLQueryItem queryItemWithName:@"resource" value:self.config[@"delcoreResource"]],
  ];
  dispatch_async(dispatch_get_main_queue(), ^{
    self.authSession = [[ASWebAuthenticationSession alloc] initWithURL:url.URL callbackURLScheme:@"anydash"
        completionHandler:^(NSURL *callback, NSError *error) {
      NSURLComponents *parts = callback ? [NSURLComponents componentsWithURL:callback resolvingAgainstBaseURL:NO] : nil;
      NSString *code = nil, *returned = nil;
      for (NSURLQueryItem *item in parts.queryItems) {
        if ([item.name isEqualToString:@"code"]) code = item.value;
        if ([item.name isEqualToString:@"state"]) returned = item.value;
      }
      if (error || !code || ![returned isEqualToString:state]) {
        completion(@"Sign-in was cancelled.");
        return;
      }
      NSUInteger generation;
      @synchronized (self) { generation = self.sessionGeneration; }
      [self tokenRequest:@{@"grant_type": @"authorization_code", @"code": code, @"redirect_uri": kRedirect, @"code_verifier": verifier}
              generation:generation
              completion:^(NSDictionary *body, NSString *failure) {
        NSString *refresh = body[@"refresh_token"];
        if (failure || ![refresh isKindOfClass:NSString.class]) { completion(failure ?: @"Sign-in returned no refresh token."); return; }
        NSString *userKey = nil;
        NSArray *parts = [body[@"id_token"] isKindOfClass:NSString.class] ? [body[@"id_token"] componentsSeparatedByString:@"."] : nil;
        if (parts.count == 3) {
          NSString *payload = [[parts[1] stringByReplacingOccurrencesOfString:@"-" withString:@"+"] stringByReplacingOccurrencesOfString:@"_" withString:@"/"];
          while (payload.length % 4) payload = [payload stringByAppendingString:@"="];
          NSData *json = [[NSData alloc] initWithBase64EncodedString:payload options:0];
          NSDictionary *claims = json ? [NSJSONSerialization JSONObjectWithData:json options:0 error:nil] : nil;
          if ([claims isKindOfClass:NSDictionary.class] && [claims[@"sub"] isKindOfClass:NSString.class])
            userKey = SHA256Hex([@"anydash:" stringByAppendingString:claims[@"sub"]]);
        }
        // A new session starts: outstanding refreshes of the previous one are discarded.
        @synchronized (self) {
          self.sessionGeneration += 1;
          [self.tokens removeAllObjects];
          KeychainSet(kRefreshAccount, refresh, nil);
          if (userKey) KeychainSet(kUserKeyAccount, userKey, nil);
        }
        completion(nil);
      }];
    }];
    self.authSession.presentationContextProvider = self;
    [self.authSession start];
  });
}

- (void)accessToken:(NSString *)resource completion:(void (^)(NSString *token, NSString *error))completion {
  NSUInteger generation;
  NSString *refresh = nil, *cachedToken = nil;
  @synchronized (self) {
    NSDictionary *cached = self.tokens[resource];
    if (cached && [cached[@"expiresAt"] doubleValue] - 60 > NSDate.date.timeIntervalSince1970) cachedToken = cached[@"token"];
    generation = self.sessionGeneration;
    if (!cachedToken) refresh = KeychainGet(kRefreshAccount, nil);
  }
  if (cachedToken) { completion(cachedToken, nil); return; }
  if (!refresh) { completion(nil, @"Sign in to AnyDash."); return; }
  [self tokenRequest:@{@"grant_type": @"refresh_token", @"refresh_token": refresh, @"resource": resource}
          generation:generation
          completion:^(NSDictionary *body, NSString *error) {
    if (error || ![body[@"access_token"] isKindOfClass:NSString.class]) { completion(nil, error ?: @"Sign in to AnyDash."); return; }
    NSTimeInterval expiresAt = NSDate.date.timeIntervalSince1970 + [body[@"expires_in"] ?: @3600 doubleValue];
    BOOL current;
    @synchronized (self) {
      // The session ended or was replaced while the request was in flight: the
      // rotated refresh token and the access token belong to a session that no
      // longer exists, so storing them would restore it.
      current = generation == self.sessionGeneration;
      if (current) {
        if ([body[@"refresh_token"] isKindOfClass:NSString.class]) KeychainSet(kRefreshAccount, body[@"refresh_token"], nil);
        self.tokens[resource] = @{@"token": body[@"access_token"], @"expiresAt": @(expiresAt)};
        if ([resource isEqualToString:self.config[@"anydashResource"]]) {
          // The widget extension may refresh while this token is valid; afterwards it
          // shows the latest snapshot. It never receives the refresh token.
          NSDictionary *shared = @{@"token": body[@"access_token"], @"expiresAt": @(expiresAt), @"version": self.config[@"version"]};
          NSData *json = [NSJSONSerialization dataWithJSONObject:shared options:0 error:nil];
          KeychainSet(kWidgetAccount, [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding], self.config[@"sharedGroup"]);
        }
      }
    }
    if (!current) {
      AnyDashLog(@"anydash_token_discarded", @{});
      completion(nil, @"Sign in to AnyDash.");
      return;
    }
    completion(body[@"access_token"], nil);
  }];
}

- (void)clearSession {
  @synchronized (self) {
    self.sessionGeneration += 1;
    [self.tokens removeAllObjects];
    KeychainRemove(kRefreshAccount, nil);
    KeychainRemove(kUserKeyAccount, nil);
    KeychainRemove(kWidgetAccount, self.config[@"sharedGroup"]);
  }
}

/// Ends the session only while `generation` is still the current one.
- (void)clearSessionIfCurrent:(NSUInteger)generation {
  @synchronized (self) {
    if (generation == self.sessionGeneration) [self clearSession];
  }
}

#pragma mark - Files

- (NSURL *)cacheDirectory {
  NSURL *base = [NSFileManager.defaultManager URLsForDirectory:NSApplicationSupportDirectory inDomains:NSUserDomainMask].firstObject;
  return [base URLByAppendingPathComponent:@"io.delino.anydash/cache" isDirectory:YES];
}
- (NSURL *)snapshotDirectory {
  NSURL *group = [NSFileManager.defaultManager containerURLForSecurityApplicationGroupIdentifier:kAppGroup];
  return [group URLByAppendingPathComponent:@"snapshots" isDirectory:YES];
}
- (NSURL *)fileForKey:(NSString *)key { return [[self cacheDirectory] URLByAppendingPathComponent:[SHA256Hex(key) stringByAppendingString:@".bin"]]; }
- (NSMutableDictionary *)cacheIndex {
  NSData *data = [NSData dataWithContentsOfURL:[[self cacheDirectory] URLByAppendingPathComponent:@"index.json"]];
  NSDictionary *index = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
  return [index isKindOfClass:NSDictionary.class] ? [index mutableCopy] : [NSMutableDictionary dictionary];
}
- (void)saveIndex:(NSDictionary *)index {
  [NSFileManager.defaultManager createDirectoryAtURL:[self cacheDirectory] withIntermediateDirectories:YES attributes:nil error:nil];
  [[NSJSONSerialization dataWithJSONObject:index options:0 error:nil] writeToURL:[[self cacheDirectory] URLByAppendingPathComponent:@"index.json"]
                                                                     options:NSDataWritingAtomic | NSDataWritingFileProtectionCompleteUntilFirstUserAuthentication error:nil];
}

#pragma mark - Dispatch

- (void)call:(NSString *)method payload:(NSString *)raw completion:(void (^)(NSString *))completion {
  NSData *json = [raw dataUsingEncoding:NSUTF8StringEncoding];
  NSDictionary *payload = json.length ? [NSJSONSerialization JSONObjectWithData:json options:0 error:nil] : @{};
  if (![payload isKindOfClass:NSDictionary.class]) { completion(Reply(nil, @"Invalid host request.")); return; }
  if ([method isEqualToString:@"info"]) {
    BOOL pad = UIDevice.currentDevice.userInterfaceIdiom == UIUserInterfaceIdiomPad;
    completion(Reply(@{@"platform": @"ios", @"collector": @NO, @"platformLabel": pad ? @"iPadOS" : @"iOS", @"hostVersion": self.config[@"version"]}, nil));
  } else if ([method isEqualToString:@"session"]) {
    NSString *refresh = KeychainGet(kRefreshAccount, nil);
    completion(Reply(refresh ? @{@"signedIn": @YES, @"userKey": KeychainGet(kUserKeyAccount, nil) ?: @""} : @{@"signedIn": @NO}, nil));
  } else if ([method isEqualToString:@"signIn"]) {
    [self signIn:^(NSString *error) { completion(Reply(@{}, error)); }];
  } else if ([method isEqualToString:@"signOut"]) {
    [self clearSession];
    [NSFileManager.defaultManager removeItemAtURL:[self cacheDirectory] error:nil];
    [NSFileManager.defaultManager removeItemAtURL:[self snapshotDirectory] error:nil];
    [AnyDashWidgetBridge reloadAll];
    completion(Reply(@{}, nil));
  } else if ([method isEqualToString:@"rpc"]) {
    [self rpc:payload completion:completion];
  } else if ([method isEqualToString:@"copyText"]) {
    dispatch_async(dispatch_get_main_queue(), ^{
      if (![payload[@"text"] isKindOfClass:NSString.class]) { completion(Reply(nil, @"Nothing to copy.")); return; }
      UIPasteboard.generalPasteboard.string = payload[@"text"];
      completion(Reply(@{}, nil));
    });
  } else if ([method isEqualToString:@"openURL"]) {
    NSURL *url = [payload[@"url"] isKindOfClass:NSString.class] ? [NSURL URLWithString:payload[@"url"]] : nil;
    if (![url.scheme.lowercaseString isEqualToString:@"https"] || !url.host.length || url.user || url.password) {
      completion(Reply(nil, @"Only HTTPS links can be opened."));
      return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
      [UIApplication.sharedApplication openURL:url options:@{} completionHandler:^(BOOL opened) {
        completion(Reply(@{}, opened ? nil : @"The link could not be opened."));
      }];
    });
  } else if ([method isEqualToString:@"cacheRead"]) {
    NSData *data = [NSData dataWithContentsOfURL:[self fileForKey:[payload[@"key"] description]]];
    completion(Reply(data ? @{@"value": [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @""} : @{}, nil));
  } else if ([method isEqualToString:@"cacheWrite"]) {
    NSString *key = [payload[@"key"] description];
    NSMutableDictionary *index = [self cacheIndex];
    index[key] = @YES;
    [self saveIndex:index];
    [[[payload[@"value"] description] dataUsingEncoding:NSUTF8StringEncoding] writeToURL:[self fileForKey:key]
        options:NSDataWritingAtomic | NSDataWritingFileProtectionCompleteUntilFirstUserAuthentication error:nil];
    completion(Reply(@{}, nil));
  } else if ([method isEqualToString:@"cacheRemove"]) {
    NSString *prefix = [payload[@"prefix"] description] ?: @"";
    NSMutableDictionary *index = [self cacheIndex];
    for (NSString *key in index.allKeys)
      if ([key hasPrefix:prefix]) {
        [NSFileManager.defaultManager removeItemAtURL:[self fileForKey:key] error:nil];
        [index removeObjectForKey:key];
      }
    [self saveIndex:index];
    completion(Reply(@{}, nil));
  } else if ([method isEqualToString:@"widgetSnapshots"]) {
    [self writeSnapshots:payload];
    completion(Reply(@{}, nil));
  } else if ([method isEqualToString:@"route"]) {
    completion(Reply(@{}, nil));
  } else {
    completion(Reply(nil, @"This action is available in the desktop app."));
  }
}

/// One binary NativeWidgetSnapshot per widget in the app group; ENDED replaces
/// the previous file so the widget removes displayed data at its next reload.
- (void)writeSnapshots:(NSDictionary *)payload {
  NSURL *directory = [self snapshotDirectory];
  if (!directory) return;
  if ([payload[@"replaceAll"] boolValue]) [NSFileManager.defaultManager removeItemAtURL:directory error:nil];
  [NSFileManager.defaultManager createDirectoryAtURL:directory withIntermediateDirectories:YES attributes:nil error:nil];
  for (NSString *encoded in payload[@"snapshots"]) {
    NSData *data = [encoded isKindOfClass:NSString.class] ? [[NSData alloc] initWithBase64EncodedString:encoded options:0] : nil;
    NSString *widgetId = data ? [AnyDashWidgetBridge widgetIdOf:data] : nil;
    if (!widgetId.length) continue;
    [data writeToURL:[directory URLByAppendingPathComponent:[SHA256Hex(widgetId) stringByAppendingString:@".pb"]]
             options:NSDataWritingAtomic | NSDataWritingFileProtectionCompleteUntilFirstUserAuthentication error:nil];
  }
  [AnyDashWidgetBridge reloadAll];
}

- (void)rpc:(NSDictionary *)payload completion:(void (^)(NSString *))completion {
  NSString *service = [payload[@"service"] description];
  NSString *method = [payload[@"method"] description];
  NSRegularExpression *name = [NSRegularExpression regularExpressionWithPattern:@"^[A-Za-z][A-Za-z0-9_.]*$" options:0 error:nil];
  if (![payload[@"target"] isEqual:@"api"] || ![service hasPrefix:@"anydash.v1."] || [service isEqualToString:@"anydash.v1.LocalCollectorService"] ||
      ![name firstMatchInString:service options:0 range:NSMakeRange(0, service.length)] ||
      ![name firstMatchInString:method options:0 range:NSMakeRange(0, method.length)]) {
    completion(Reply(nil, @"Invalid RPC target."));
    return;
  }
  NSData *body = [[NSData alloc] initWithBase64EncodedString:[payload[@"body"] description] ?: @"" options:0] ?: [NSData data];
  void (^send)(NSString *, NSData *) = ^(NSString *token, NSData *requestBody) {
    NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"%@/%@/%@", self.config[@"api"], service, method]];
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url];
    request.HTTPMethod = @"POST";
    request.HTTPBody = requestBody;
    [request setValue:@"application/proto" forHTTPHeaderField:@"Content-Type"];
    [request setValue:@"1" forHTTPHeaderField:@"Connect-Protocol-Version"];
    NSDictionary *headers = [payload[@"headers"] isKindOfClass:NSDictionary.class] ? payload[@"headers"] : @{};
    for (NSString *key in headers)
      if ([@[@"x-anydash-client-kind", @"x-anydash-client-version"] containsObject:key.lowercaseString] && [headers[key] isKindOfClass:NSString.class])
        [request setValue:headers[key] forHTTPHeaderField:key];
    if (token) [request setValue:[@"Bearer " stringByAppendingString:token] forHTTPHeaderField:@"Authorization"];
    [[NSURLSession.sharedSession dataTaskWithRequest:request completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
      if (error) { completion(Reply(nil, @"The network request failed.")); return; }
      NSHTTPURLResponse *http = (NSHTTPURLResponse *)response;
      NSString *correlation = [http valueForHTTPHeaderField:@"X-AnyDash-Correlation-Id"];
      if (http.statusCode != 200) AnyDashLog(@"anydash_rpc_failed", @{@"method": method, @"status": @(http.statusCode), @"correlationId": correlation ?: @""});
      completion(Reply(@{@"status": @(http.statusCode), @"body": [data base64EncodedStringWithOptions:0] ?: @"", @"correlationId": correlation ?: @""}, nil));
    }] resume];
  };
  if (!KeychainGet(kRefreshAccount, nil)) { send(nil, body); return; }
  [self accessToken:self.config[@"anydashResource"] completion:^(NSString *token, NSString *error) {
    if (!token) { completion(Reply(@{@"status": @401, @"body": @""}, nil)); return; }
    NSString *target = [NSString stringWithFormat:@"%@/%@", service, method];
    if (![@[@"anydash.v1.AccountService/ConnectAccount", @"anydash.v1.DashboardService/ListSpaces"] containsObject:target]) {
      send(token, body);
      return;
    }
    // These requests carry `string delcore_access_token = 1`; the host appends
    // the Delcore token so it never crosses into the Lynx bundle.
    [self accessToken:self.config[@"delcoreResource"] completion:^(NSString *delcore, NSString *failure) {
      if (!delcore) { completion(Reply(@{@"status": @401, @"body": @""}, nil)); return; }
      send(token, WithDelcoreToken(body, delcore));
    }];
  }];
}
@end

@implementation AnyDashHostModule
+ (NSString *)name { return @"AnyDashHost"; }
+ (NSDictionary *)methodLookup { return @{@"call": NSStringFromSelector(@selector(call:payload:callback:))}; }
- (void)call:(NSString *)method payload:(NSString *)payload callback:(LynxCallbackBlock)callback {
  [[AnyDashHost shared] call:method payload:payload completion:^(NSString *reply) { callback(reply); }];
}
@end
