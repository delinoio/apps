// SPDX-License-Identifier: Apache-2.0
#import "LaneReauthentication.h"
@implementation LaneReauthentication {
  NSDictionary *_binding;
  NSString *_proof;
  NSDate *_expires;
}
- (void)beginSpace:(NSString *)space account:(NSString *)account mode:(NSString *)mode challenge:(NSString *)challenge {
  [self cancel:nil];
  if (!space.length || !account.length || !mode.length || !challenge.length) return;
  _binding = @{@"spaceId": space, @"accountId": account, @"mode": mode, @"challengeId": challenge};
  _expires = [NSDate dateWithTimeIntervalSinceNow:300];
}
- (NSDictionary *)bindingForChallenge:(NSString *)challenge selectedSpace:(NSString *)space {
  if (!_binding || [_expires timeIntervalSinceNow] <= 0 || ![_binding[@"challengeId"] isEqual:challenge] || ![_binding[@"spaceId"] isEqual:space]) return nil;
  return _binding;
}
- (BOOL)acceptProof:(NSString *)proof challenge:(NSString *)challenge selectedSpace:(NSString *)space {
  if (![proof isKindOfClass:NSString.class] || !proof.length || ![self bindingForChallenge:challenge selectedSpace:space] || _proof) return NO;
  _proof = [proof copy]; return YES;
}
- (NSString *)consumeForSpace:(NSString *)space account:(NSString *)account mode:(NSString *)mode challenge:(NSString *)challenge {
  NSDictionary *binding = [self bindingForChallenge:challenge selectedSpace:space];
  if (!binding || ![binding[@"accountId"] isEqual:account] || ![binding[@"mode"] isEqual:mode] || !_proof) return nil;
  NSString *proof = _proof; [self cancel:nil]; return proof;
}
- (void)cancel:(NSString *)challenge {
  if (!challenge || [_binding[@"challengeId"] isEqual:challenge]) { _binding = nil; _proof = nil; _expires = nil; }
}
@end
