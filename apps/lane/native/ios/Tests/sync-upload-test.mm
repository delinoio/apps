// SPDX-License-Identifier: Apache-2.0
// Executes the iOS upload coordinator against a disposable native store and
// deterministic transport replies; no account or device is contacted.
#import "LaneSyncUpload.h"
#include "lane_store.h"
#include <cassert>
#include <string>

static NSString *Encode(id object) {
  return [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:object options:0 error:nil]
                         encoding:NSUTF8StringEncoding];
}
static NSDictionary *Decode(const std::string &value) {
  return [NSJSONSerialization JSONObjectWithData:[NSData dataWithBytes:value.data() length:value.size()]
    options:0 error:nil];
}
static std::string UTF8(NSString *value) { return std::string(value.UTF8String); }
static NSDictionary *Success(const std::string &value) {
  NSDictionary *envelope = Decode(value);
  if (![envelope[@"ok"] boolValue])
    fprintf(stderr, "Unexpected native store error: %s\n", [envelope[@"error"][@"reason"] UTF8String]);
  assert([envelope[@"ok"] boolValue]);
  return envelope[@"response"];
}
static NSString *ID(NSUInteger value) {
  return [NSString stringWithFormat:@"01900000-0000-7000-8000-%012lx", (unsigned long)value];
}
static NSDictionary *Mutation(NSUInteger number, NSUInteger length) {
  NSString *entity = ID(number);
  return @{@"kind": @"MUTATION_KIND_CREATE", @"meta": @{@"requestId": ID(number + 10000)},
    @"version": @{@"entityId": entity, @"versionId": ID(number + 20000), @"kind": @"ENTITY_KIND_TASK",
      @"task": @{@"title": @"Fixture", @"description": [@"x" stringByPaddingToLength:length withString:@"x" startingAtIndex:0]}}};
}
static NSDictionary *Pending(NSArray *mutations) {
  return @{@"formatVersion": @3, @"epoch": ID(2), @"deviceId": ID(3), @"mutations": mutations};
}

static void MockCases(void) {
  NSDictionary *info = @{@"maxItemBytes": @"4096"};
  NSUInteger limit = 4096 * 1000 + 65536;
  __block NSMutableArray<NSDictionary *> *queue = [NSMutableArray arrayWithObjects:Mutation(10, 3000),
    Mutation(11, 5000), Mutation(12, 3000), nil];
  NSMutableArray *sent = [NSMutableArray new];
  NSMutableArray *rejected = [NSMutableArray new];
  NSString *failure = LaneUploadPending(Pending(queue), info,
    ^NSArray *{ return [queue copy]; },
    ^NSDictionary *(NSArray *items) {
      for (NSDictionary *item in items) [sent addObject:item[@"version"][@"entityId"]];
      NSMutableArray *results = [NSMutableArray new];
      for (NSDictionary *item in items) {
        NSMutableDictionary *result = [@{@"requestId": item[@"meta"][@"requestId"]} mutableCopy];
        if ([item[@"version"][@"entityId"] isEqual:ID(11)])
          result[@"error"] = @{@"reason": @"ERROR_REASON_ITEM_TOO_LARGE", @"retryable": @YES};
        [results addObject:result];
      }
      return @{@"results": results};
    },
    ^BOOL(NSDictionary *response) {
      for (NSDictionary *result in response[@"results"])
        for (NSDictionary *item in [queue copy]) if ([result[@"requestId"] isEqual:item[@"meta"][@"requestId"]]) {
          if (result[@"error"]) [rejected addObject:item[@"version"][@"entityId"]];
          [queue removeObject:item];
        }
      return YES;
    });
  assert(!failure && queue.count == 0 && sent.count == 3 && rejected.count == 1);
  assert([rejected[0] isEqual:ID(11)]);

  queue = [NSMutableArray new];
  for (NSUInteger i = 0; i < 1500; ++i) [queue addObject:Mutation(i + 100, 3000)];
  __block NSUInteger calls = 0;
  failure = LaneUploadPending(Pending(queue), info,
    ^NSArray *{ return [queue copy]; },
    ^NSDictionary *(NSArray *items) {
      ++calls;
      assert(items.count <= 1000);
      assert([Encode(Pending(items)) lengthOfBytesUsingEncoding:NSUTF8StringEncoding] <= limit);
      NSMutableArray *results = [NSMutableArray new];
      for (NSDictionary *item in items) [results addObject:@{@"requestId": item[@"meta"][@"requestId"]}];
      return @{@"results": results};
    },
    ^BOOL(NSDictionary *response) {
      NSSet *ids = [NSSet setWithArray:[response[@"results"] valueForKey:@"requestId"]];
      NSIndexSet *matched = [queue indexesOfObjectsPassingTest:^BOOL(NSDictionary *item, NSUInteger index, BOOL *stop) {
        return [ids containsObject:item[@"meta"][@"requestId"]];
      }];
      [queue removeObjectsAtIndexes:matched];
      return YES;
    });
  assert(!failure && !queue.count && calls >= 2);

  queue = [NSMutableArray arrayWithObjects:Mutation(20, 3000), Mutation(21, 3000), Mutation(22, 3000), nil];
  __block BOOL rejectedOnce = NO;
  failure = LaneUploadPending(Pending(queue), info,
    ^NSArray *{ return [queue copy]; },
    ^NSDictionary *(NSArray *items) {
      for (NSDictionary *item in items) if ([item[@"version"][@"entityId"] isEqual:ID(21)])
        return @{@"error": @{@"reason": @"ERROR_REASON_ITEM_TOO_LARGE", @"httpStatus": @413}};
      NSMutableArray *results = [NSMutableArray new];
      for (NSDictionary *item in items) [results addObject:@{@"requestId": item[@"meta"][@"requestId"]}];
      return @{@"results": results};
    },
    ^BOOL(NSDictionary *response) {
      for (NSDictionary *result in response[@"results"])
        for (NSDictionary *item in [queue copy]) if ([result[@"requestId"] isEqual:item[@"meta"][@"requestId"]]) {
          rejectedOnce |= result[@"error"] != nil;
          [queue removeObject:item];
        }
      return YES;
    });
  assert(!failure && rejectedOnce && !queue.count);

  NSMutableDictionary *marginal = [Mutation(23, 3000) mutableCopy];
  NSMutableDictionary *version = [marginal[@"version"] mutableCopy];
  NSMutableDictionary *body = [version[@"task"] mutableCopy];
  body[@"prompt"] = [@"y" stringByPaddingToLength:3000 withString:@"y" startingAtIndex:0];
  version[@"task"] = body;
  marginal[@"version"] = version;
  queue = [NSMutableArray arrayWithObjects:marginal, Mutation(24, 30), nil];
  __block BOOL serverRejected = NO;
  failure = LaneUploadPending(Pending(queue), info,
    ^NSArray *{ return [queue copy]; },
    ^NSDictionary *(NSArray *items) {
      NSMutableArray *results = [NSMutableArray new];
      for (NSDictionary *item in items) {
        NSMutableDictionary *result = [@{@"requestId": item[@"meta"][@"requestId"]} mutableCopy];
        if ([item[@"version"][@"entityId"] isEqual:ID(23)])
          result[@"error"] = @{@"reason": @"ERROR_REASON_ITEM_TOO_LARGE", @"retryable": @YES};
        [results addObject:result];
      }
      return @{@"results": results};
    },
    ^BOOL(NSDictionary *response) {
      for (NSDictionary *result in response[@"results"])
        for (NSDictionary *item in [queue copy]) if ([result[@"requestId"] isEqual:item[@"meta"][@"requestId"]]) {
          serverRejected |= result[@"error"] != nil;
          [queue removeObject:item];
        }
      return YES;
    });
  assert(!failure && serverRejected && !queue.count);

  queue = [NSMutableArray arrayWithObjects:Mutation(25, 5000), Mutation(26, 30), nil];
  __block BOOL replayed = NO;
  __block NSUInteger replayCalls = 0;
  failure = LaneUploadPending(Pending(queue), info,
    ^NSArray *{ return [queue copy]; },
    ^NSDictionary *(NSArray *items) {
      ++replayCalls;
      assert(items.count == 2);
      assert([items[0][@"version"][@"entityId"] isEqual:ID(25)]);
      assert([items[1][@"version"][@"entityId"] isEqual:ID(26)]);
      return @{@"results": @[
        @{@"requestId": items[0][@"meta"][@"requestId"], @"result": @"RESULT_KIND_REPLAYED"},
        @{@"requestId": items[1][@"meta"][@"requestId"], @"result": @"RESULT_KIND_APPLIED"}
      ]};
    },
    ^BOOL(NSDictionary *response) {
      for (NSDictionary *result in response[@"results"])
        for (NSDictionary *item in [queue copy]) if ([result[@"requestId"] isEqual:item[@"meta"][@"requestId"]]) {
          replayed |= [result[@"result"] isEqual:@"RESULT_KIND_REPLAYED"];
          [queue removeObject:item];
        }
      return YES;
    });
  assert(!failure && replayed && replayCalls == 1 && !queue.count);

  for (NSString *reason in @[@"RATE_LIMITED", @"UNAUTHENTICATED", @"STALE_EPOCH", @"DEPENDENCY_UNAVAILABLE"]) {
    __block BOOL applied = NO;
    failure = LaneUploadPending(Pending(@[Mutation(30, 30)]), info,
      ^NSArray *{ return @[]; },
      ^NSDictionary *(NSArray *items) { return @{@"error": @{@"reason": [@"ERROR_REASON_" stringByAppendingString:reason],
        @"httpStatus": [reason isEqual:@"RATE_LIMITED"] ? @429 : @503}}; },
      ^BOOL(NSDictionary *response) { applied = YES; return YES; });
    assert([failure isEqual:[@"ERROR_REASON_" stringByAppendingString:reason]] && !applied);
  }
  __block BOOL appliedTransient = NO;
  NSDictionary *throttled = Mutation(31, 30);
  failure = LaneUploadPending(Pending(@[throttled]), info,
    ^NSArray *{ return @[]; },
    ^NSDictionary *(NSArray *items) { return @{@"results": @[@{@"requestId": throttled[@"meta"][@"requestId"],
      @"error": @{@"reason": @"ERROR_REASON_RATE_LIMITED", @"retryable": @YES}}]}; },
    ^BOOL(NSDictionary *response) { appliedTransient = YES; return YES; });
  assert([failure isEqual:@"ERROR_REASON_RATE_LIMITED"] && !appliedTransient);
  puts("PASS: bounded batches, 413 isolation, independent progress and transient failures");
}

static void StoreCase(void) {
  NSString *root = [NSTemporaryDirectory() stringByAppendingPathComponent:[@"lane-ios-sync-" stringByAppendingString:NSUUID.UUID.UUIDString]];
  assert([[NSFileManager defaultManager] createDirectoryAtPath:root withIntermediateDirectories:YES attributes:nil error:nil]);
  {
    lane::Store storage(UTF8(root));
    lane::Store *store = &storage;
    NSDictionary *opened = Success(store->open(R"({"formatVersion":3,"spaceId":""})"));
    NSString *space = opened[@"spaceId"], *device = opened[@"deviceId"];
    __block NSUInteger next = 100000;
    NSDictionary *(^meta)(void) = ^NSDictionary *{
      return @{@"requestId": ID(next++), @"origin": @{@"kind": @"ORIGIN_KIND_APP",
        @"deviceId": device, @"deviceName": @"Fixture"}};
    };
    NSDictionary *(^task)(NSString *, NSString *) = ^NSDictionary *(NSString *title, NSString *prompt) {
      return Success(store->invoke("CreateTask", UTF8(Encode(@{@"spaceId": space,
        @"request": @{@"meta": meta(), @"id": ID(next++),
          @"repository": @{@"url": @"https://example.test/repo"}, @"title": title, @"prompt": prompt}}))))[@"result"][@"version"];
    };
    NSDictionary *large = task(@"Large", [@"x" stringByPaddingToLength:4200000 withString:@"x" startingAtIndex:0]);
    NSMutableDictionary *descendantBody = [large[@"task"] mutableCopy];
    descendantBody[@"title"] = @"Large revision";
    NSDictionary *descendant = Success(store->invoke("SaveTask", UTF8(Encode(@{@"spaceId": space,
      @"request": @{@"meta": meta(), @"id": large[@"entityId"],
        @"expectedVersionId": large[@"versionId"], @"task": descendantBody}}))))[@"result"][@"version"];
    NSDictionary *small = task(@"Small", @"small");
    NSString *epoch = ID(next++), *owner = ID(next++);
    NSString *account = ID(next++), *tenant = ID(next++);
    Success(store->approveAccount(UTF8(Encode(@{@"issuer": @"https://identity.example.test",
      @"accountId": account, @"tenantId": tenant, @"epoch": epoch})), true));
    space = Success(store->open(R"({"formatVersion":3,"spaceId":""})"))[@"spaceId"];
    void (^renew)(void) = ^{
      assert([Success(store->acquireSync(UTF8(space), UTF8(owner)))[@"acquired"] boolValue]);
    };
    NSDictionary *(^current)(void) = ^NSDictionary *{
      renew();
      return Success(store->pendingSync(UTF8(space), UTF8(owner)));
    };
    NSDictionary *pending = current();
    NSMutableArray *sent = [NSMutableArray new];
    LanePendingMutations reload = ^NSArray *{
      return current()[@"mutations"];
    };
    LaneApplyPush apply = ^BOOL(NSDictionary *response) {
      renew();
      NSDictionary *request = @{@"formatVersion": @3, @"epoch": epoch, @"owner": owner, @"push": response};
      return [Decode(store->applySync(UTF8(space), UTF8(Encode(request))))[@"ok"] boolValue];
    };
    LanePushMutations push = ^NSDictionary *(NSArray *items) {
      NSMutableArray *results = [NSMutableArray new];
      for (NSDictionary *item in items) {
        [sent addObject:item[@"version"][@"entityId"]];
        [results addObject:@{@"requestId": item[@"meta"][@"requestId"]}];
      }
      return @{@"results": results};
    };
    NSString *failure = LaneUploadPending(pending, @{@"maxItemBytes": @"4096"}, reload, push, apply);
    assert(!failure && [sent containsObject:small[@"entityId"]] && ![sent containsObject:large[@"entityId"]]);
    NSDictionary *status = Success(store->invoke("GetLocalStatus", UTF8(Encode(@{@"spaceId": space, @"request": @{}}))));
    BOOL visible = NO;
    for (NSDictionary *item in status[@"itemErrors"])
      visible |= [item[@"entityId"] isEqual:large[@"entityId"]] &&
        [item[@"reason"] isEqual:@"ERROR_REASON_ITEM_TOO_LARGE"];
    assert(visible);
    assert([current()[@"mutations"] count] == 0);
    assert(!LaneUploadPending(current(),
      @{@"maxItemBytes": @"4096"}, reload, push, apply));
    assert(![sent containsObject:large[@"entityId"]]);
    NSString *remoteID = ID(next++), *remoteVersionID = ID(next++);
    NSMutableDictionary *remoteVersion = [small mutableCopy];
    NSMutableDictionary *remoteBody = [small[@"task"] mutableCopy];
    remoteBody[@"title"] = @"Remote update";
    remoteVersion[@"entityId"] = remoteID;
    remoteVersion[@"versionId"] = remoteVersionID;
    remoteVersion[@"task"] = remoteBody;
    NSDictionary *remote = @{@"formatVersion": @3, @"epoch": epoch, @"owner": owner,
      @"pull": @{@"changes": @[@{@"sequence": @"1", @"entity": @{@"result": @"RESULT_KIND_APPLIED",
        @"version": remoteVersion}}], @"nextCursor": @"fixture-cursor"}};
    renew();
    assert([Decode(store->applySync(UTF8(space), UTF8(Encode(remote))))[@"ok"] boolValue]);
    assert([current()[@"cursor"] isEqual:@"fixture-cursor"]);
    NSDictionary *downloaded = Success(store->invoke("GetTask", UTF8(Encode(@{@"spaceId": space,
      @"request": @{@"id": remoteID}}))))[@"result"][@"version"];
    assert([downloaded[@"task"][@"title"] isEqual:@"Remote update"]);
    NSDictionary *stored = Success(store->invoke("GetTask", UTF8(Encode(@{@"spaceId": space,
      @"request": @{@"id": large[@"entityId"]}}))))[@"result"][@"version"];
    assert([stored[@"task"][@"prompt"] length] == 4200000);
    NSMutableDictionary *correctedBody = [descendant[@"task"] mutableCopy];
    correctedBody[@"title"] = @"Corrected";
    correctedBody[@"prompt"] = @"short";
    NSDictionary *corrected = Success(store->invoke("SaveTask", UTF8(Encode(@{@"spaceId": space,
      @"request": @{@"meta": meta(), @"id": large[@"entityId"],
        @"expectedVersionId": descendant[@"versionId"], @"task": correctedBody}}))))[@"result"][@"version"];
    assert(![corrected[@"versionId"] isEqual:descendant[@"versionId"]]);
    NSArray *correctedPending = current()[@"mutations"];
    assert(correctedPending.count == 1);
    assert([correctedPending[0][@"version"][@"versionId"] isEqual:corrected[@"versionId"]]);
    assert([correctedPending[0][@"kind"] isEqual:@"MUTATION_KIND_CREATE"]);
    Success(store->invoke("RetryBlockedItems", UTF8(Encode(@{@"spaceId": space, @"request": @{}}))));
    assert(!LaneUploadPending(current(),
      @{@"maxItemBytes": @"4096"}, reload, push, apply));
    assert([sent containsObject:large[@"entityId"]]);
    assert([current()[@"mutations"] count] == 0);
  }
  assert([[NSFileManager defaultManager] removeItemAtPath:root error:nil]);
  puts("PASS: native store retains oversized content and durable visible item error");
}

int main(void) {
  @autoreleasepool { MockCases(); StoreCase(); }
}
