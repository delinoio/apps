// SPDX-License-Identifier: Apache-2.0
// Runs on macOS against the real Foundation/Security code, with a mocked server.
#import "LaneCredentials.h"
#include <cassert>

@interface MockTransport : LaneTransport
@property(nonatomic) NSUInteger exchanges;
@property(nonatomic, copy) NSString *device;
@property(nonatomic, copy) NSString *account;
@property(nonatomic, strong) NSDictionary *lastForm;
@end
@implementation MockTransport
- (NSDictionary *)postForm:(NSDictionary *)form url:(NSURL *)url {
  self.exchanges++; self.lastForm = form;
  assert([url.absoluteString isEqualToString:@"https://lane.invalid/oauth/token"]);
  assert([form[@"grant_type"] isEqualToString:@"authorization_code"]);
  assert([form[@"code_verifier"] length] >= 43);
  return @{@"access_token": @"synthetic-access", @"token_type": @"Bearer", @"expires_in": @3600,
    @"refresh_token": @"synthetic-refresh", @"issuer": @"https://identity.invalid", @"account_id": self.account,
    @"tenant_id": @"tenant", @"device_id": self.device, @"session_id": @"session"};
}
- (NSDictionary *)callService:(NSString *)service method:(NSString *)method request:(NSDictionary *)request token:(NSString *)token {
  assert([token isEqualToString:@"synthetic-access"]);
  assert([service isEqualToString:@"lane.v1.SyncService"]);
  assert([method isEqualToString:@"GetSyncInfo"]);
  return @{@"formatVersion": @1, @"accountId": self.account, @"tenantId": @"tenant", @"epoch": @"epoch"};
}
@end
static NSURL *Callback(NSDictionary *challenge, NSString *suffix) {
  return [NSURL URLWithString:[NSString stringWithFormat:@"lane-dev://auth/callback?state=%@&code=synthetic-code%@", challenge[@"challengeId"], suffix]];
}
int main(void) {
  @autoreleasepool {
    NSDictionary *configuration = @{@"LaneAPIURL": @"https://lane.invalid", @"LaneOAuthClientID": @"test-client", @"LaneURLScheme": @"lane-dev"};
    MockTransport *server = [MockTransport new]; server.device = LaneUUID7(); server.account = @"account";
    LaneCredentials *auth = [[LaneCredentials alloc] initWithTransport:server configuration:configuration];
    NSDictionary *challenge = [auth begin:@"account" device:server.device];
    NSURLComponents *url = [NSURLComponents componentsWithString:challenge[@"authorizationUrl"]];
    NSMutableDictionary *parameters = [NSMutableDictionary new];
    for (NSURLQueryItem *item in url.queryItems) parameters[item.name] = item.value;
    assert([url.host isEqualToString:@"lane.invalid"]);
    assert([parameters[@"code_challenge_method"] isEqualToString:@"S256"]);
    assert([parameters[@"device_id"] isEqual:server.device]);
    assert([parameters[@"redirect_uri"] isEqualToString:@"lane-dev://auth/callback"]);
    assert([parameters[@"code_challenge"] length] == 43);
    assert(![auth complete:[NSURL URLWithString:@"lane-dev://auth/callback?state=wrong&code=code"]]);
    assert(![auth complete:Callback(challenge, @"&state=duplicate")]);
    assert(![auth complete:[NSURL URLWithString:[[Callback(challenge, @"") absoluteString] stringByReplacingOccurrencesOfString:@"lane-dev:" withString:@"lane:"]]]);
    assert(server.exchanges == 0);
    NSDictionary *approved = [auth complete:Callback(challenge, @"")];
    assert([approved[@"identity"][@"accountId"] isEqualToString:@"account"]);
    assert([approved[@"identity"][@"deviceId"] isEqual:server.device]);
    assert(server.exchanges == 1);
    assert(![auth complete:Callback(challenge, @"")]); // One-use callback.
    challenge = [auth begin:@"other-account" device:server.device];
    assert(![auth complete:Callback(challenge, @"")]);
    challenge = [auth begin:@"account" device:LaneUUID7()];
    assert(![auth complete:Callback(challenge, @"")]); // Server cannot replace device identity.
    challenge = [auth begin:@"account" device:server.device];
    assert(![auth complete:Callback(challenge, @"&error=access_denied")]);
    assert(![auth complete:Callback(challenge, @"")]);
    LaneCredentials *unconfigured = [[LaneCredentials alloc] initWithTransport:server configuration:@{}];
    assert([unconfigured begin:@"" device:server.device][@"error"]);
    LaneCredentials *insecure = [[LaneCredentials alloc] initWithTransport:server configuration:@{@"LaneAPIURL": @"http://lane.invalid", @"LaneOAuthClientID": @"test", @"LaneURLScheme": @"lane-dev"}];
    assert([insecure begin:@"" device:server.device][@"error"]);
    for (int i = 0; i < 100; ++i) {
      NSString *uuid = LaneUUID7(); assert(uuid.length == 36); assert([uuid characterAtIndex:14] == '7');
      assert([[NSCharacterSet characterSetWithCharactersInString:@"89ab"] characterIsMember:[uuid characterAtIndex:19]]);
    }
    assert([LaneJSON(LaneFailure(@"UNAUTHENTICATED", NO))[@"error"][@"reason"] isEqualToString:@"ERROR_REASON_UNAUTHENTICATED"]);
    const unsigned char detailBytes[] = {8, 5, 18, 3, 'r', 'e', 'q', 40, 1};
    NSData *detail = [NSData dataWithBytes:detailBytes length:sizeof(detailBytes)];
    NSDictionary *connect = @{@"code": @"aborted", @"details": @[@{@"type": @"lane.v1.ErrorDetail", @"value": [detail base64EncodedStringWithOptions:0]}]};
    NSDictionary *failure = LaneConnectFailure([LaneEncode(connect) dataUsingEncoding:NSUTF8StringEncoding], 409);
    assert([failure[@"error"][@"reason"] isEqualToString:@"ERROR_REASON_STALE_RESOLUTION"]);
    assert([failure[@"error"][@"correlationId"] isEqualToString:@"req"]);
    assert([failure[@"error"][@"retryable"] boolValue]);
    assert([LaneConnectFailure([@"not-json" dataUsingEncoding:NSUTF8StringEncoding], 401)[@"error"][@"reason"] isEqualToString:@"ERROR_REASON_UNAUTHENTICATED"]);
    puts("PASS: native OAuth state, PKCE, callback replay, account/device isolation, configuration, UUID v7, typed errors (mock server)");
  }
}
