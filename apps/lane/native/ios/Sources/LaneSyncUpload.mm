// SPDX-License-Identifier: Apache-2.0
#import "LaneSyncUpload.h"

static const NSUInteger LaneUploadCallLimit = 20;
static const NSUInteger LaneUploadItemLimit = 1000;

static NSUInteger JSONBytes(id value) {
  NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
  return data.length;
}

static NSUInteger RequestBytes(NSDictionary *pending, NSArray *mutations) {
  return JSONBytes(@{@"formatVersion": @3, @"epoch": pending[@"epoch"],
    @"deviceId": pending[@"deviceId"], @"mutations": mutations});
}

static BOOL IsDeletion(NSDictionary *item) {
  return [item[@"kind"] isEqual:@"MUTATION_KIND_DELETE"] ||
    [item[@"version"][@"deleted"] boolValue];
}

static BOOL KnownOversized(NSDictionary *item, NSUInteger itemLimit) {
  if (IsDeletion(item)) return NO;
  // These fields are retained by server normalization. Their UTF-8 bytes alone
  // prove the canonical version exceeds the limit. Marginal cases go to the
  // server for its authoritative normalized per-item result.
  NSDictionary *body = item[@"version"][@"task"];
  for (NSString *field in @[@"title", @"description", @"prompt"])
    if ([body[field] isKindOfClass:NSString.class] &&
        [body[field] lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > itemLimit)
      return YES;
  return NO;
}

static NSArray<NSString *> *References(NSDictionary *item) {
  NSMutableArray<NSString *> *references = [NSMutableArray new];
  NSDictionary *version = item[@"version"];
  for (NSString *bodyName in @[@"task", @"urlConnection", @"label", @"milestone", @"cycle"]) {
    NSDictionary *body = version[bodyName];
    if (![body isKindOfClass:NSDictionary.class]) continue;
    for (NSString *field in @[@"repositoryId", @"milestoneId", @"cycleId", @"parentTaskId", @"duplicateOfTaskId"])
      if ([body[field] isKindOfClass:NSString.class]) [references addObject:body[field]];
    for (NSString *field in @[@"labelIds", @"dependsOnTaskIds", @"relatedTaskIds"])
      if ([body[field] isKindOfClass:NSArray.class])
        for (id value in body[field]) if ([value isKindOfClass:NSString.class]) [references addObject:value];
  }
  return references;
}

static BOOL IndependentCreate(NSDictionary *candidate, NSArray<NSDictionary *> *batch) {
  if (![candidate[@"kind"] isEqual:@"MUTATION_KIND_CREATE"] || !candidate[@"version"][@"task"])
    return NO;
  NSString *entity = candidate[@"version"][@"entityId"];
  NSArray *references = References(candidate);
  for (NSDictionary *selected in batch) {
    if (![selected[@"kind"] isEqual:@"MUTATION_KIND_CREATE"] || !selected[@"version"][@"task"])
      return NO;
    NSString *other = selected[@"version"][@"entityId"];
    if ([entity isEqual:other] || [references containsObject:other] || [References(selected) containsObject:entity])
      return NO;
  }
  return YES;
}

static NSDictionary *TooLargeResult(NSDictionary *item) {
  return @{@"results": @[@{@"requestId": item[@"meta"][@"requestId"],
    @"error": @{@"reason": @"ERROR_REASON_ITEM_TOO_LARGE", @"retryable": @NO}}]};
}

static BOOL AuthoritativeItemRejection(NSDictionary *error) {
  return [error[@"reason"] isEqual:@"ERROR_REASON_ITEM_TOO_LARGE"];
}

static BOOL TransientItemError(NSDictionary *error) {
  NSString *reason = error[@"reason"];
  if (AuthoritativeItemRejection(error)) return NO;
  return [error[@"retryable"] boolValue] || [@[@"ERROR_REASON_RATE_LIMITED",
    @"ERROR_REASON_DEPENDENCY_UNAVAILABLE", @"ERROR_REASON_UNAUTHENTICATED",
    @"ERROR_REASON_FORBIDDEN", @"ERROR_REASON_STALE_EPOCH",
    @"ERROR_REASON_LEASE_INVALID", @"ERROR_REASON_ONLINE_REQUIRED"] containsObject:reason];
}

// A true restart means the store must recompute dependency holdbacks before
// another request. In particular, do not send a child from a failed batch.
static NSString *SendBatch(NSArray<NSDictionary *> *items, LanePushMutations push,
    LaneApplyPush apply, NSUInteger *calls, BOOL *restart) {
  if (*calls >= LaneUploadCallLimit) return nil;
  ++*calls;
  NSDictionary *response = push(items);
  NSDictionary *error = response[@"error"];
  if (error) {
    if ([error[@"httpStatus"] integerValue] != 413)
      return error[@"reason"] ?: @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
    if (items.count == 1) {
      if (!apply(TooLargeResult(items.firstObject))) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
      *restart = YES;
      return nil;
    }
    NSUInteger middle = items.count / 2;
    NSString *failure = SendBatch([items subarrayWithRange:NSMakeRange(0, middle)], push, apply, calls, restart);
    if (failure || *restart || *calls >= LaneUploadCallLimit) return failure;
    return SendBatch([items subarrayWithRange:NSMakeRange(middle, items.count - middle)], push, apply, calls, restart);
  }
  NSArray *results = response[@"results"];
  if (![results isKindOfClass:NSArray.class] || !results.count)
    return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  NSMutableSet *expected = [NSMutableSet new];
  for (NSDictionary *item in items) [expected addObject:item[@"meta"][@"requestId"]];
  NSMutableArray *durable = [NSMutableArray new];
  NSString *transient = nil;
  for (NSDictionary *result in results) {
    NSString *requestID = result[@"requestId"];
    if (![expected containsObject:requestID]) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
    [expected removeObject:requestID];
    if (TransientItemError(result[@"error"]))
      transient = result[@"error"][@"reason"] ?: @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
    else {
      [durable addObject:result];
      if (result[@"error"]) *restart = YES;
    }
  }
  if (expected.count) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  if (durable.count && !apply(@{@"results": durable,
      @"repositoryMappings": response[@"repositoryMappings"] ?: @[]}))
    return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  if (!durable.count && !transient) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  return transient;
}

NSString *LaneUploadPending(NSDictionary *pending, NSDictionary *info,
    LanePendingMutations reload, LanePushMutations push, LaneApplyPush apply) {
  unsigned long long advertised = 0;
  id raw = info[@"maxItemBytes"];
  if ([raw isKindOfClass:NSString.class] || [raw isKindOfClass:NSNumber.class]) {
    NSString *text = [raw description];
    NSScanner *scanner = [NSScanner scannerWithString:text];
    if (![scanner scanUnsignedLongLong:&advertised] || !scanner.isAtEnd) advertised = 0;
  }
  if (!advertised || advertised > (NSUIntegerMax - 65536) / 1000)
    return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  NSUInteger itemLimit = (NSUInteger)advertised;
  NSUInteger requestLimit = itemLimit * 1000 + 65536;
  NSArray<NSDictionary *> *items = pending[@"mutations"];
  NSUInteger calls = 0;
  NSUInteger decisions = 0;
  while (items.count && calls < LaneUploadCallLimit && decisions < LaneUploadCallLimit) {
    NSDictionary *first = items.firstObject;
    if (KnownOversized(first, itemLimit) || RequestBytes(pending, @[first]) > requestLimit) {
      if (!apply(TooLargeResult(first))) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
      ++decisions;
      items = reload();
      if (!items) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
      continue;
    }
    NSMutableArray<NSDictionary *> *batch = [NSMutableArray arrayWithObject:first];
    NSUInteger bytes = RequestBytes(pending, batch);
    for (NSUInteger index = 1; index < items.count && batch.count < LaneUploadItemLimit; ++index) {
      NSDictionary *candidate = items[index];
      if (KnownOversized(candidate, itemLimit) || !IndependentCreate(candidate, batch)) break;
      NSUInteger addition = JSONBytes(candidate) + 1; // JSON array comma.
      if (addition > requestLimit - bytes) break;
      [batch addObject:candidate];
      bytes += addition;
    }
    BOOL restart = NO;
    NSString *failure = SendBatch(batch, push, apply, &calls, &restart);
    if (failure) return failure;
    ++decisions;
    items = reload();
    if (!items) return @"ERROR_REASON_DEPENDENCY_UNAVAILABLE";
  }
  return nil;
}
