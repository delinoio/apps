// SPDX-License-Identifier: Apache-2.0
#import "LaneHost.h"
#import "LaneLog.h"
#import "LaneCredentials.h"
#import "LaneWatch.h"
#import "LaneReauthentication.h"
#import "LaneDeletion.h"
#import <BackgroundTasks/BackgroundTasks.h>
#import <UIKit/UIKit.h>
#include "lane_store.h"
#include <memory>
#include <stdexcept>

static NSString *const RefreshID = @"io.delino.lane.refresh";
static std::string UTF8(NSString *value) { return value ? std::string(value.UTF8String) : std::string(); }
static NSString *String(const std::string &value) { return [[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding]; }
static NSDictionary *Response(const std::string &value) { return LaneJSON(String(value))[@"response"]; }
static NSString *Permission(UNNotificationSettings *settings) {
  switch (settings.authorizationStatus) {
    case UNAuthorizationStatusAuthorized:
    case UNAuthorizationStatusProvisional:
    case UNAuthorizationStatusEphemeral: return @"NOTIFICATION_PERMISSION_GRANTED";
    case UNAuthorizationStatusDenied: return @"NOTIFICATION_PERMISSION_DENIED";
    default: return @"NOTIFICATION_PERMISSION_NOT_REQUESTED";
  }
}
static BOOL UUID7(NSString *value) {
  if (![value isKindOfClass:NSString.class]) return NO;
  NSRegularExpression *expression = [NSRegularExpression regularExpressionWithPattern:@"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$" options:0 error:nil];
  return [expression numberOfMatchesInString:value options:0 range:NSMakeRange(0,value.length)] == 1;
}

@implementation LaneHost {
  std::unique_ptr<lane::Store> _store;
  dispatch_queue_t _storage;
  dispatch_queue_t _network;
  dispatch_source_t _timer;
  LaneTransport *_transport;
  LaneCredentials *_credentials;
  NSMutableSet<NSString *> *_knownSpaces;
  NSMutableDictionary<NSString *, NSNumber *> *_handles;
  NSString *_owner;
  NSString *_deviceID;
  NSString *_permission;
  NSString *_pushToken;
  NSString *_pushState;
  NSDictionary *_pendingRoute;
  NSMutableDictionary *_authorizationContext;
  LaneReauthentication *_reauthentication;
  NSString *_selectedSpace;
  NSUInteger _selectionGeneration;
  NSUInteger _reauthGeneration;
  BOOL _bridgeReady;
  BOOL _active;
  BOOL _networkAllowed;
  BOOL _syncRunning;
  NSMutableArray *_syncCompletions;
  NSMutableDictionary<NSString *, LaneWatch *> *_watches;
  BOOL _started;
  NSUInteger _ticks;
}
+ (instancetype)shared {
  static LaneHost *host; static dispatch_once_t once;
  dispatch_once(&once, ^{ host = [LaneHost new]; }); return host;
}
- (instancetype)init {
  if ((self = [super init])) {
    _storage = dispatch_queue_create("io.delino.lane.storage", DISPATCH_QUEUE_SERIAL);
    _network = dispatch_queue_create("io.delino.lane.network", DISPATCH_QUEUE_SERIAL);
    _transport = [LaneTransport new]; _credentials = [[LaneCredentials alloc] initWithTransport:_transport];
    _reauthentication = [LaneReauthentication new];
    _syncCompletions = [NSMutableArray new];
    _watches = [NSMutableDictionary new];
    _knownSpaces = [NSMutableSet new]; _handles = [NSMutableDictionary new];
    _owner = LaneUUID7(); _permission = @"NOTIFICATION_PERMISSION_NOT_REQUESTED";
    _pushState = @"NOT_REGISTERED";
  } return self;
}
- (BOOL)ensureStore {
  if (_store) return YES;
  try {
    NSURL *support = [NSFileManager.defaultManager URLForDirectory:NSApplicationSupportDirectory inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
    if (!support || !_owner) return NO;
    NSURL *root = [support URLByAppendingPathComponent:@"io.delino.lane" isDirectory:YES];
    NSError *error;
    if (![NSFileManager.defaultManager createDirectoryAtURL:root withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700, NSFileProtectionKey: NSFileProtectionCompleteUntilFirstUserAuthentication} error:&error]) return NO;
    _store = std::make_unique<lane::Store>(UTF8(root.path));
    __weak LaneHost *weakSelf = self;
    lane::HostHooks hooks;
    hooks.remote = [weakSelf](std::string space, std::string method, std::string request) {
      LaneHost *host = weakSelf;
      if (!host) return UTF8(LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES));
      NSString *name = String(method);
      NSString *service = [@[@"ListDevices", @"DisconnectDevice", @"ListConnections", @"RevokeConnection", @"RegisterPush", @"BeginDataDeletion", @"DeleteAccountData"] containsObject:name] ? @"lane.v1.AccountService" : @"lane.v1.WebhookService";
      return UTF8(LaneEncode([host->_transport callService:service method:name request:LaneJSON(String(request)) ?: @{} token:[host->_credentials tokenForSpace:String(space)] ?: @""]));
    };
    hooks.authorize = [weakSelf](std::string account) {
      LaneHost *host = weakSelf;
      NSDictionary *challenge = [host->_credentials begin:String(account) device:host->_deviceID];
      host->_authorizationContext[@"challengeId"] = challenge[@"challengeId"] ?: @"";
      return UTF8(LaneEncode(challenge));
    };
    hooks.requestNotificationPermission = [weakSelf]() {
      LaneHost *host = weakSelf;
      return UTF8(host ? host->_permission : @"NOTIFICATION_PERMISSION_UNSUPPORTED");
    };
    hooks.disconnect = [weakSelf](std::string space) {
      LaneHost *host = weakSelf;
      NSDictionary *status = Response(host->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": String(space), @"request": @{}}))));
      if ([status[@"syncEnabled"] boolValue]) [host->_credentials disconnect:String(space)];
      else [host->_credentials forget:String(space)];
      if ([host->_selectedSpace isEqual:String(space)]) { [host->_reauthentication cancel:nil]; ++host->_selectionGeneration; }
      [host->_knownSpaces removeObject:String(space)];
      [host->_watches[String(space)] cancel]; [host->_watches removeObjectForKey:String(space)];
    };
    _store->setHostHooks(std::move(hooks));
    return YES;
  } catch (...) { _store.reset(); return NO; }
}
- (void)reply:(LynxCallbackBlock)callback value:(NSString *)value {
  dispatch_async(dispatch_get_main_queue(), ^{ callback(value ?: LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)); });
}
- (void)start {
  if (_started) return; _started = YES;
  UNUserNotificationCenter.currentNotificationCenter.delegate = self;
  BOOL registered = [BGTaskScheduler.sharedScheduler registerForTaskWithIdentifier:RefreshID usingQueue:dispatch_get_main_queue() launchHandler:^(__kindof BGTask *task) {
    [self scheduleRefresh];
    dispatch_async(self->_storage, ^{ self->_networkAllowed = YES; });
    __block BOOL completed = NO;
    void (^finish)(BOOL) = ^(BOOL success) {
      dispatch_async(dispatch_get_main_queue(), ^{
        if (completed) return; completed = YES;
        task.expirationHandler = nil;
        [self endBackgroundOpportunity];
        [task setTaskCompletedWithSuccess:success];
      });
    };
    task.expirationHandler = ^{ [self endBackgroundOpportunity]; finish(NO); };
    [self syncWithCompletion:finish];
  }];
  if (!registered) LaneLog(@"lane_background_registration_failed");
  _timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, _storage);
  dispatch_source_set_timer(_timer, DISPATCH_TIME_NOW, NSEC_PER_SEC / 2, NSEC_PER_SEC / 20);
  dispatch_source_set_event_handler(_timer, ^{
    if (!self->_active) return;
    if ([self->_authorizationContext[@"expires"] doubleValue] < NSDate.date.timeIntervalSince1970) self->_authorizationContext = nil;
    [self poll];
    ++self->_ticks;
    if (self->_ticks % 10 == 0) for (NSString *space in self->_knownSpaces) [self deliverNotifications:space];
    if (self->_ticks % 60 == 0) {
      [self syncWithCompletion:^(BOOL ignored) {}];
      for (NSString *space in self->_knownSpaces) [self updatePushRegistration:space];
    }
  });
  dispatch_resume(_timer);
  [self foreground];
}
- (void)scheduleRefresh {
  BGAppRefreshTaskRequest *request = [[BGAppRefreshTaskRequest alloc] initWithIdentifier:RefreshID];
  request.earliestBeginDate = [NSDate dateWithTimeIntervalSinceNow:15 * 60];
  NSError *error;
  if (![BGTaskScheduler.sharedScheduler submitTaskRequest:request error:&error])
    LaneLog(@"lane_background_schedule_unavailable");
}
- (void)foreground {
  dispatch_async(_storage, ^{ self->_active = YES; self->_networkAllowed = YES; [self poll]; });
  [self refreshPermission:nil];
  [self syncWithCompletion:^(BOOL ignored) {}];
}
- (void)background {
  dispatch_async(_storage, ^{
    self->_active = NO;
    for (LaneWatch *watch in self->_watches.allValues) [watch cancel];
    [self->_watches removeAllObjects];
  });
  [self scheduleRefresh];
  // iOS grants only bounded time. Expiration cancels network work and completes once.
  __block UIBackgroundTaskIdentifier identifier = UIBackgroundTaskInvalid;
  void (^finish)(void) = ^{
    dispatch_async(dispatch_get_main_queue(), ^{
      if (identifier != UIBackgroundTaskInvalid) {
        [UIApplication.sharedApplication endBackgroundTask:identifier]; identifier = UIBackgroundTaskInvalid;
        [self endBackgroundOpportunity];
      }
    });
  };
  identifier = [UIApplication.sharedApplication beginBackgroundTaskWithName:@"Lane synchronization" expirationHandler:^{ [self endBackgroundOpportunity]; finish(); }];
  [self syncWithCompletion:^(BOOL ignored) { finish(); }];
}
- (void)endBackgroundOpportunity {
  dispatch_async(_storage, ^{
    if (!self->_active) { self->_networkAllowed = NO; [self->_transport cancel]; }
  });
}
- (void)poll {
  if (![self ensureStore]) return;
  try {
    NSData *data = [String(_store->poll()) dataUsingEncoding:NSUTF8StringEncoding];
    id changes = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![changes isKindOfClass:NSArray.class]) return;
    for (NSDictionary *change in changes) {
      if (!UUID7(change[@"spaceId"])) continue;
      [_knownSpaces addObject:change[@"spaceId"]];
      NSDictionary *event = @{@"spaceId": change[@"spaceId"], @"sequence": change[@"sequence"] ?: @"0"};
      dispatch_async(dispatch_get_main_queue(), ^{ [self.view sendGlobalEvent:@"lane.storeChanged" withParams:@[event]]; });
      [self deliverNotifications:change[@"spaceId"]];
    }
  } catch (...) { LaneLog(@"lane_store_poll_failed"); }
}
- (NSString *)attachment:(NSDictionary *)request space:(NSString *)space {
  NSString *local = String(_store->invoke("GetAttachment", UTF8(LaneEncode(@{@"spaceId": space, @"request": request}))));
  NSDictionary *envelope = LaneJSON(local);
  if (![envelope[@"ok"] boolValue] || ![envelope[@"response"][@"state"] isEqual:@"ATTACHMENT_STATE_WAITING_FOR_SYNC"]) return local;
  NSDictionary *status = Response(_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
  NSDictionary *identity = [_credentials identityForSpace:space];
  if (![status[@"syncEnabled"] boolValue] || [status[@"reconnectRequired"] boolValue] || !identity) return local;
  if (![identity[@"accountId"] isEqual:status[@"accountId"]] || ![identity[@"deviceId"] isEqual:status[@"deviceId"]]) return LaneFailure(@"FORBIDDEN", NO);
  NSString *owner = LaneUUID7();
  if (![Response(_store->acquireSync(UTF8(space), UTF8(owner)))[@"acquired"] boolValue]) return LaneFailure(@"STORAGE_BUSY", YES);
  NSDictionary *pending;
  try { pending = Response(_store->pendingSync(UTF8(space), UTF8(owner))); }
  catch (...) { _store->acquireSync(UTF8(space), UTF8(owner), 0); throw; }
  _store->acquireSync(UTF8(space), UTF8(owner), 0);
  NSString *token = [_credentials tokenForSpace:space];
  if (!token.length) return LaneFailure(@"UNAUTHENTICATED", NO);
  NSDictionary *info = [_transport callService:@"lane.v1.SyncService" method:@"GetSyncInfo" request:@{} token:token];
  if (info[@"error"]) return LaneEncode(info);
  if ([info[@"formatVersion"] integerValue] != 3) return LaneFailure(@"UNSUPPORTED_FORMAT", NO);
  if (![info[@"accountId"] isEqual:identity[@"accountId"]] || ![info[@"tenantId"] isEqual:identity[@"tenantId"]]) return LaneFailure(@"FORBIDDEN", NO);
  if (![info[@"epoch"] isEqual:pending[@"epoch"]]) return LaneFailure(@"STALE_EPOCH", NO);
  NSDictionary *remote = [_transport callService:@"lane.v1.CaptureService" method:@"GetAttachment" request:request token:token];
  if (remote[@"error"]) return LaneEncode(remote);
  if ([@[@"ATTACHMENT_STATE_WAITING_FOR_SYNC", @"ATTACHMENT_STATE_DELETED"] containsObject:remote[@"state"]])
    return LaneEncode(@{@"ok": @YES, @"response": @{@"state": remote[@"state"]}, @"sequence": envelope[@"sequence"] ?: @"0"});
  if (![remote[@"state"] isEqual:@"ATTACHMENT_STATE_AVAILABLE"]) return LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES);
  NSDictionary *download = [remote[@"png"] isKindOfClass:NSString.class] && [remote[@"png"] length] ? remote :
    [_transport attachment:[NSURL URLWithString:[remote[@"downloadUrl"] isKindOfClass:NSString.class] ? remote[@"downloadUrl"] : @""]];
  if (download[@"error"]) return LaneEncode(download);
  // Cache only inside the original account space. No shared files or signed URLs
  // cross the renderer boundary; the engine revalidates the owning manifest.
  if (![[_credentials identityForSpace:space] isEqual:identity]) return LaneFailure(@"FORBIDDEN", NO);
  return String(_store->cacheAttachment(UTF8(space), UTF8(request[@"taskId"]), UTF8(request[@"attachmentId"]), UTF8(download[@"png"])));
}
- (void)open:(NSString *)json completion:(LynxCallbackBlock)completion {
  if (![json isKindOfClass:NSString.class]) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
  dispatch_async(_storage, ^{
    if (![self ensureStore]) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; return; }
    try {
      NSString *result = String(self->_store->open(UTF8(json)));
      NSString *space = LaneJSON(result)[@"response"][@"spaceId"];
      self->_deviceID = LaneJSON(result)[@"response"][@"deviceId"] ?: self->_deviceID;
      if (UUID7(space)) {
        if (![self->_selectedSpace isEqual:space]) {
          [self->_reauthentication cancel:nil]; ++self->_selectionGeneration; self->_selectedSpace = space;
        }
        [self->_knownSpaces addObject:space];
        self->_handles[space] = @([self->_handles[space] integerValue] + 1);
        NSDictionary *preferences = Response(self->_store->invoke("GetNotificationPreferences", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
        if (![preferences[@"permission"] isEqual:self->_permission]) self->_store->recordNotificationPermission(UTF8(space), UTF8(self->_permission));
      }
      [self reply:completion value:result];
      dispatch_async(dispatch_get_main_queue(), ^{
        self->_bridgeReady = YES;
        if (self->_pendingRoute) {
          [self.view sendGlobalEvent:@"lane.openRoute" withParams:@[self->_pendingRoute]];
          self->_pendingRoute = nil;
        }
      });
      [self poll];
    } catch (...) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; }
  });
}
- (void)invoke:(NSString *)method json:(NSString *)json completion:(LynxCallbackBlock)completion {
  if (![method isKindOfClass:NSString.class] || ![json isKindOfClass:NSString.class]) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
  NSDictionary *request = LaneJSON(json);
  if ([method isEqualToString:@"SetNotificationPreferences"] && [request[@"request"] isKindOfClass:NSDictionary.class] && [request[@"request"][@"enabled"] isKindOfClass:NSNumber.class] && [request[@"request"][@"enabled"] boolValue]) {
    dispatch_async(dispatch_get_main_queue(), ^{
      [UNUserNotificationCenter.currentNotificationCenter requestAuthorizationWithOptions:UNAuthorizationOptionAlert | UNAuthorizationOptionSound | UNAuthorizationOptionBadge completionHandler:^(BOOL granted, NSError *error) {
        [self refreshPermission:^{ [self invokeReady:method json:json completion:completion]; }];
      }];
    });
  } else [self invokeReady:method json:json completion:completion];
}
- (void)invokeReady:(NSString *)method json:(NSString *)json completion:(LynxCallbackBlock)completion {
  dispatch_async(_storage, ^{
    if (![self ensureStore]) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; return; }
    NSDictionary *request = LaneJSON(json);
    NSString *space = request[@"spaceId"];
    if ([method isEqualToString:@"ResetDevelopmentData"]) {
      // Explicit development reset. It works without an open handle so an
      // unsupported store can be replaced; the engine takes the exclusive
      // lifetime lease and fails with STORAGE_BUSY while any handle remains.
      if (![request[@"request"] isKindOfClass:NSDictionary.class] || ([space isKindOfClass:NSString.class] && space.length && !UUID7(space))) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
      [self->_reauthentication cancel:nil]; ++self->_selectionGeneration;
      if ([space isKindOfClass:NSString.class] && space.length) { [self->_watches[space] cancel]; [self->_watches removeObjectForKey:space]; }
      else { for (LaneWatch *watch in self->_watches.allValues) [watch cancel]; [self->_watches removeAllObjects]; }
      try { [self reply:completion value:String(self->_store->invoke("ResetDevelopmentData", UTF8(json)))]; [self poll]; }
      catch (...) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; }
      return;
    }
    if (![method isKindOfClass:NSString.class] || ![request[@"request"] isKindOfClass:NSDictionary.class] || !UUID7(space) || !self->_handles[space]) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
    if ([@[@"CreateCaptureTask", @"CreateURLConnection", @"SaveURLConnection", @"DeleteURLConnection"] containsObject:method]) {
      [self reply:completion value:LaneFailure(@"FORBIDDEN", NO)]; return;
    }
    try {
      if ([method isEqualToString:@"ConnectSync"]) {
        NSDictionary *statusEnvelope = LaneJSON(String(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}})))));
        NSDictionary *status = statusEnvelope[@"response"];
        // The UI asks for explicit retained-data upload confirmation before a
        // new ConnectSync when reconnectRequired is true. Initial login is not consent.
        BOOL reconnect = [status[@"reconnectRequired"] boolValue];
        self->_authorizationContext = [@{@"spaceId": space, @"accountId": status[@"accountId"] ?: @"",
          @"sequence": statusEnvelope[@"sequence"] ?: @"", @"reconnect": @(reconnect), @"expires": @(NSDate.date.timeIntervalSince1970 + 600)} mutableCopy];
        if (reconnect) {
          if (![request[@"request"][@"accountId"] isEqual:status[@"accountId"]] ||
              ![Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) {
            self->_authorizationContext = nil; [self reply:completion value:LaneFailure(@"STALE_EPOCH", NO)]; return;
          }
          NSDictionary *pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
          self->_authorizationContext[@"epoch"] = pending[@"epoch"] ?: @"";
          self->_authorizationContext[@"cursor"] = pending[@"cursor"] ?: @"";
          self->_store->acquireSync(UTF8(space), UTF8(self->_owner), 0);
          [self->_watches[space] cancel]; [self->_watches removeObjectForKey:space];
        }
      }
      NSString *invokeJSON = json;
      if ([method isEqual:@"BeginDataDeletion"]) {
        [self->_reauthentication cancel:nil]; ++self->_selectionGeneration;
        if (![space isEqual:self->_selectedSpace]) { [self reply:completion value:LaneFailure(@"FORBIDDEN", NO)]; return; }
      }
      if ([method isEqual:@"DeleteAccountData"]) {
        NSMutableDictionary *body = [request[@"request"] mutableCopy];
        NSString *challenge = body[@"challengeId"];
        NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
        if (body[@"reauthProof"] || body[@"reauth_proof"] || ![challenge isKindOfClass:NSString.class] ||
            ![space isEqual:self->_selectedSpace] || self->_reauthGeneration != self->_selectionGeneration ||
            ![body[@"confirmedAccountId"] isEqual:status[@"accountId"]]) {
          [self reply:completion value:LaneFailure(@"FORBIDDEN", NO)]; return;
        }
        NSString *proof = [self->_reauthentication consumeForSpace:space account:status[@"accountId"] mode:body[@"mode"] challenge:challenge];
        if (!proof) { [self reply:completion value:LaneFailure(@"UNAUTHENTICATED", NO)]; return; }
        [body removeObjectForKey:@"challengeId"]; body[@"reauthProof"] = proof;
        invokeJSON = LaneEncode(@{@"spaceId": space, @"request": body});
      }
      // Store assigns APP and the pinned account/device. UI attribution is never trusted.
      NSString *result = [method isEqual:@"GetAttachment"] ? [self attachment:request[@"request"] space:space] : String(self->_store->invoke(UTF8(method), UTF8(invokeJSON)));
      if ([method isEqual:@"BeginDataDeletion"] && [LaneJSON(result)[@"ok"] boolValue]) {
        NSString *challenge = LaneJSON(result)[@"response"][@"challengeId"];
        NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
        if ([challenge isKindOfClass:NSString.class] && [request[@"request"][@"mode"] isKindOfClass:NSString.class]) {
          [self->_reauthentication beginSpace:space account:status[@"accountId"] mode:request[@"request"][@"mode"] challenge:challenge];
          self->_reauthGeneration = self->_selectionGeneration;
        }
      }
      if ([method isEqual:@"ConnectSync"] && ![LaneJSON(result)[@"ok"] boolValue]) self->_authorizationContext = nil;
      if ([method isEqual:@"ConnectSync"] && [LaneJSON(result)[@"ok"] boolValue]) {
        NSURL *url = [NSURL URLWithString:LaneJSON(result)[@"response"][@"authorizationUrl"] ?: @""];
        if ([url.scheme.lowercaseString isEqual:@"https"] && url.host.length && !url.user && !url.password)
          dispatch_async(dispatch_get_main_queue(), ^{ [UIApplication.sharedApplication openURL:url options:@{} completionHandler:nil]; });
      }
      [self reply:completion value:result]; [self poll];
      if ([method isEqualToString:@"SetNotificationPreferences"] && [LaneJSON(result)[@"ok"] boolValue]) {
        [self updatePushRegistration:space];
        dispatch_async(dispatch_get_main_queue(), ^{
          if ([self->_permission isEqualToString:@"NOTIFICATION_PERMISSION_GRANTED"]) [UIApplication.sharedApplication registerForRemoteNotifications];
        });
      }
    } catch (...) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; }
  });
}
- (void)close:(NSString *)space completion:(LynxCallbackBlock)completion {
  dispatch_async(_storage, ^{
    if (![self ensureStore] || !UUID7(space)) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
    if ([space isEqual:self->_selectedSpace]) {
      [self->_reauthentication cancel:nil]; ++self->_selectionGeneration; self->_selectedSpace = nil;
    }
    NSInteger count = [self->_handles[space] integerValue];
    if (count > 1) self->_handles[space] = @(count - 1);
    else [self->_handles removeObjectForKey:space];
    try { [self reply:completion value:String(self->_store->close(UTF8(space)))]; }
    catch (...) { [self reply:completion value:LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES)]; }
  });
}
- (void)cancelReauthentication:(NSString *)challenge completion:(LynxCallbackBlock)completion {
  if (![challenge isKindOfClass:NSString.class] || !challenge.length) { [self reply:completion value:LaneFailure(@"INVALID_INPUT", NO)]; return; }
  dispatch_async(_storage, ^{
    [self->_reauthentication cancel:challenge];
    [self reply:completion value:LaneEncode(@{@"ok": @YES})];
  });
}
- (void)refreshPermission:(dispatch_block_t)completion {
  [UNUserNotificationCenter.currentNotificationCenter getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
    dispatch_async(self->_storage, ^{
      self->_permission = Permission(settings);
      if ([self ensureStore]) for (NSString *space in self->_knownSpaces) {
        try {
          NSDictionary *preferences = Response(self->_store->invoke("GetNotificationPreferences", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
          if (![preferences[@"permission"] isEqual:self->_permission]) self->_store->recordNotificationPermission(UTF8(space), UTF8(self->_permission));
          if ([preferences[@"enabled"] boolValue] && [self->_permission isEqualToString:@"NOTIFICATION_PERMISSION_GRANTED"])
            dispatch_async(dispatch_get_main_queue(), ^{ [UIApplication.sharedApplication registerForRemoteNotifications]; });
        } catch (...) {}
      }
      if (completion) completion();
    });
  }];
}
- (void)syncFailure:(NSString *)reason space:(NSString *)space {
  dispatch_sync(_storage, ^{
    try { self->_store->recordSyncFailure(UTF8(space), UTF8(reason ?: @"ERROR_REASON_DEPENDENCY_UNAVAILABLE")); [self poll]; } catch (...) {}
  });
}
// Runs on the network worker. Revoked credentials are used only on this minimal
// recovery endpoint, never to retrieve account content or authorize a new epoch.
- (BOOL)recoverDeletion:(NSString *)space {
  NSString *token = [self->_credentials lastAccessTokenForSpace:space];
  if (!token.length) return NO;
  __block NSDictionary *pending; __block NSDictionary *status;
  dispatch_sync(_storage, ^{
    try {
      if (![Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) return;
      pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
      status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
    } catch (...) {}
  });
  if (!pending) return NO;
  NSDictionary *directive = [_transport getPath:@"/auth/deletion-status" query:@{} token:token];
  LaneDeletionDisposition disposition = LaneValidateDeletion(directive, status[@"accountId"], pending[@"deviceId"], pending[@"epoch"]);
  if (![[self->_credentials deviceForSpace:space] isEqual:pending[@"deviceId"]]) disposition = LaneDeletionInvalid;
  __block BOOL handled = NO;
  dispatch_sync(_storage, ^{
    try {
      NSDictionary *current = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
      if ([current[@"epoch"] isEqual:pending[@"epoch"]] && [Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) {
        if (disposition == LaneDeletionErase) {
          NSDictionary *pull = @{@"changes": @[@{@"deletion": @{@"kind": @"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT", @"id": status[@"accountId"], @"deletionVersionId": directive[@"new_epoch"]}}], @"nextCursor": @"", @"hasMore": @NO};
          handled = [LaneJSON(String(self->_store->applySync(UTF8(space), UTF8(LaneEncode(@{@"formatVersion": @3, @"epoch": pending[@"epoch"], @"owner": self->_owner, @"pull": pull})))))[@"ok"] boolValue];
        } else if (disposition == LaneDeletionRetain) {
          handled = [LaneJSON(String(self->_store->recordSyncFailure(UTF8(space), "ERROR_REASON_STALE_EPOCH")))[@"ok"] boolValue];
          [self->_watches[space] cancel]; [self->_watches removeObjectForKey:space];
        }
      }
      self->_store->acquireSync(UTF8(space), UTF8(self->_owner), 0);
      [self poll];
    } catch (...) {}
  });
  return handled;
}
- (void)syncWithCompletion:(void (^)(BOOL))completion { [self syncOnlySpaces:nil completion:completion]; }
- (void)syncOnlySpaces:(NSArray<NSString *> *)requested completion:(void (^)(BOOL))completion {
  dispatch_async(_storage, ^{
    if (!self->_networkAllowed) { dispatch_async(dispatch_get_main_queue(), ^{ completion(NO); }); return; }
    if (self->_syncRunning) {
      dispatch_async(self->_network, ^{ [self syncOnlySpaces:requested completion:completion]; });
      return;
    }
    [self->_syncCompletions addObject:[completion copy]];
    self->_syncRunning = YES;
    [self->_transport resume];
    [self poll];
    NSMutableArray *spaces = [NSMutableArray new];
    NSMutableDictionary *devices = [NSMutableDictionary new];
    for (NSString *space in self->_knownSpaces) {
      if (requested && ![requested containsObject:space]) continue;
      if ([self->_authorizationContext[@"reconnect"] boolValue] && [self->_authorizationContext[@"spaceId"] isEqual:space]) continue;
      try {
        NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
        if ([status[@"syncEnabled"] boolValue] && ![status[@"reconnectRequired"] boolValue]) { [spaces addObject:space]; devices[space] = status[@"deviceId"] ?: @""; }
      } catch (...) {}
    }
    dispatch_async(self->_network, ^{
      BOOL success = YES;
      for (NSString *space in spaces) {
        NSString *credentialDevice = [self->_credentials deviceForSpace:space];
        if (credentialDevice && ![credentialDevice isEqual:devices[space]]) {
          [self syncFailure:@"ERROR_REASON_FORBIDDEN" space:space]; success = NO; continue;
        }
        NSString *token = [self->_credentials tokenForSpace:space];
        if (!token.length) {
          if (![self recoverDeletion:space]) [self syncFailure:@"ERROR_REASON_UNAUTHENTICATED" space:space]; success = NO; continue;
        }
        __block BOOL acquired = NO;
        dispatch_sync(self->_storage, ^{
          try { acquired = [Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]; } catch (...) {}
        });
        if (!acquired) continue;
        BOOL more = YES;
        // Bound each opportunity; the next run resumes from the durable cursor.
        for (NSUInteger page = 0; more && page < 20; ++page) {
          __block NSDictionary *pending;
          dispatch_sync(self->_storage, ^{
            try {
              if ([Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue])
                pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
            } catch (...) {}
          });
          if (!pending || [pending[@"formatVersion"] integerValue] != 3) { [self syncFailure:@"ERROR_REASON_UNSUPPORTED_FORMAT" space:space]; success = NO; break; }
          NSDictionary *info = [self->_transport callService:@"lane.v1.SyncService" method:@"GetSyncInfo" request:@{} token:token];
          if (info[@"error"]) { if (![info[@"error"][@"reason"] isEqual:@"ERROR_REASON_UNAUTHENTICATED"] || ![self recoverDeletion:space]) [self syncFailure:info[@"error"][@"reason"] space:space]; success = NO; break; }
          if (![info[@"formatVersion"] respondsToSelector:@selector(integerValue)] || [info[@"formatVersion"] integerValue] != 3) {
            [self syncFailure:@"ERROR_REASON_UNSUPPORTED_FORMAT" space:space]; success = NO; break;
          }
          BOOL epochChanged = ![info[@"epoch"] isEqual:pending[@"epoch"]];
          // Never push old-generation data. An authenticated old-epoch pull may
          // carry an explicit ACCOUNT deletion; STALE_EPOCH alone never erases.
          if (!epochChanged && [pending[@"mutations"] count]) {
            NSDictionary *push = [self->_transport callService:@"lane.v1.SyncService" method:@"PushChanges"
              request:@{@"formatVersion": @3, @"epoch": pending[@"epoch"], @"deviceId": pending[@"deviceId"], @"mutations": pending[@"mutations"]} token:token];
            if (push[@"error"]) { [self syncFailure:push[@"error"][@"reason"] space:space]; success = NO; break; }
            __block BOOL applied = NO;
            dispatch_sync(self->_storage, ^{
              try {
              if (![Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) return;
              applied = [LaneJSON(String(self->_store->applySync(UTF8(space), UTF8(LaneEncode(@{@"formatVersion": @3, @"epoch": pending[@"epoch"], @"owner": self->_owner, @"push": push})))))[@"ok"] boolValue]; } catch (...) {}
            });
            if (!applied) { success = NO; break; }
          }
          NSDictionary *pull = [self->_transport callService:@"lane.v1.SyncService" method:@"PullChanges"
            request:@{@"formatVersion": @3, @"epoch": pending[@"epoch"], @"cursor": pending[@"cursor"] ?: @"", @"pageSize": @100} token:token];
          if (pull[@"error"]) { [self syncFailure:pull[@"error"][@"reason"] space:space]; success = NO; break; }
          __block BOOL applied = NO;
          dispatch_sync(self->_storage, ^{
            try {
              if (![Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) return;
              applied = [LaneJSON(String(self->_store->applySync(UTF8(space), UTF8(LaneEncode(@{@"formatVersion": @3, @"epoch": pending[@"epoch"], @"owner": self->_owner, @"pull": pull})))))[@"ok"] boolValue]; [self poll]; } catch (...) {}
          });
          if (!applied) { success = NO; break; }
          more = !epochChanged && [pull[@"hasMore"] boolValue];
          if (epochChanged) break;
        }
        dispatch_sync(self->_storage, ^{
          try {
            NSDictionary *pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
            NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
            if (self->_active && [status[@"syncEnabled"] boolValue] && ![status[@"reconnectRequired"] boolValue] && !self->_watches[space] && pending[@"epoch"]) {
              __weak LaneHost *weakSelf = self;
              LaneWatch *watch = [[LaneWatch alloc] initWithToken:token epoch:pending[@"epoch"] cursor:pending[@"cursor"] ?: @""
                changed:^{ [weakSelf syncWithCompletion:^(BOOL ignored) {}]; }
                ended:^{
                  LaneHost *host = weakSelf;
                  if (host) dispatch_async(host->_storage, ^{ [host->_watches removeObjectForKey:space]; });
                }];
              if (watch) self->_watches[space] = watch;
            }
            self->_store->acquireSync(UTF8(space), UTF8(self->_owner), 0);
          } catch (...) {}
        });
      }
      dispatch_async(self->_storage, ^{
        self->_syncRunning = NO;
        NSArray *callbacks = [self->_syncCompletions copy]; [self->_syncCompletions removeAllObjects];
        dispatch_async(dispatch_get_main_queue(), ^{ for (void (^callback)(BOOL) in callbacks) callback(success); });
      });
    });
  });
}
- (void)deliverNotifications:(NSString *)space {
  if (![_permission isEqualToString:@"NOTIFICATION_PERMISSION_GRANTED"]) return;
  try {
    NSArray *items = Response(_store->claimNotifications(UTF8(space), UTF8(_owner)))[@"items"];
    for (NSDictionary *item in items) {
      NSString *proposal = item[@"proposalId"], *device = item[@"deviceId"];
      if (!UUID7(proposal) || !UUID7(device)) continue;
      NSString *identifier = [NSString stringWithFormat:@"%@/%@", proposal, device];
      UNMutableNotificationContent *content = [UNMutableNotificationContent new];
      BOOL generic = [item[@"content"] isEqual:@"NOTIFICATION_CONTENT_GENERIC"] || [item[@"content"] isEqual:@2];
      content.title = generic ? @"Lane" : (item[@"repositoryName"] ?: @"Lane");
      content.body = generic ? @"A new task proposal is ready to review." : (item[@"title"] ?: @"A new task proposal is ready to review.");
      content.sound = UNNotificationSound.defaultSound;
      content.userInfo = @{@"spaceId": space, @"taskId": proposal};
      UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:identifier content:content trigger:nil];
      [UNUserNotificationCenter.currentNotificationCenter addNotificationRequest:request withCompletionHandler:^(NSError *error) {
        if (error) { LaneLog(@"lane_notification_schedule_failed"); return; }
        dispatch_async(self->_storage, ^{
          try { self->_store->acknowledgeNotification(UTF8(space), UTF8(proposal)); } catch (...) {}
        });
      }];
    }
  } catch (...) { LaneLog(@"lane_notification_read_failed"); }
}
- (void)registeredPushToken:(NSData *)token {
  const unsigned char *bytes = (const unsigned char *)token.bytes;
  NSMutableString *value = [NSMutableString new];
  for (NSUInteger i = 0; i < token.length; ++i) [value appendFormat:@"%02x", bytes[i]];
  dispatch_async(_storage, ^{
    self->_pushToken = value; self->_pushState = @"REGISTERED";
    for (NSString *space in self->_knownSpaces) [self updatePushRegistration:space];
  });
}
- (void)pushRegistrationFailed {
  dispatch_async(_storage, ^{
    self->_pushState = @"UNSUPPORTED";
    dispatch_async(dispatch_get_main_queue(), ^{
      [self.view sendGlobalEvent:@"lane.pushStatus" withParams:@[@{@"supported": @NO, @"message": @"Remote notifications are unavailable. Local Lane features remain available."}]];
    });
  });
}
- (void)updatePushRegistration:(NSString *)space {
  if (!_pushToken.length) return;
  try {
    NSDictionary *status = Response(_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
    NSDictionary *preferences = status[@"notificationPreferences"];
    if (!preferences || !status[@"deviceId"]) return;
    NSDictionary *request = @{@"meta": @{@"requestId": LaneUUID7()}, @"deviceId": status[@"deviceId"],
      @"provider": @"PUSH_PROVIDER_APNS", @"endpoint": _pushToken,
      @"enabled": @([preferences[@"enabled"] boolValue] && [_permission isEqualToString:@"NOTIFICATION_PERMISSION_GRANTED"]),
      @"content": preferences[@"content"] ?: @"NOTIFICATION_CONTENT_DETAIL"};
    dispatch_async(_network, ^{
      NSString *token = [self->_credentials tokenForSpace:space];
      if (!token.length) return;
      NSDictionary *result = [self->_transport callService:@"lane.v1.AccountService" method:@"RegisterPush" request:request token:token];
      if (result[@"error"]) LaneLog(@"lane_push_registration_pending");
    });
  } catch (...) {}
}
- (void)receivePush:(NSDictionary *)payload completion:(void (^)(BOOL))completion {
  // Data-only APNs is an untrusted hint, never a current-space routing command.
  NSString *account = payload[@"accountId"];
  if (![payload[@"type"] isEqual:@"lane.proposal"] || !UUID7(payload[@"proposalId"]) ||
      ![account isKindOfClass:NSString.class] || !account.length) { completion(NO); return; }
  dispatch_async(_storage, ^{
    if (![self ensureStore]) { dispatch_async(dispatch_get_main_queue(), ^{ completion(NO); }); return; }
    try {
      NSArray *spaces = Response(self->_store->spacesForAccount(UTF8(account)))[@"items"];
      if (![spaces isKindOfClass:NSArray.class] || !spaces.count) { dispatch_async(dispatch_get_main_queue(), ^{ completion(NO); }); return; }
      [self->_knownSpaces addObjectsFromArray:spaces];
      self->_networkAllowed = YES;
      [self syncOnlySpaces:spaces completion:^(BOOL success) { [self endBackgroundOpportunity]; completion(success); }];
    } catch (...) { dispatch_async(dispatch_get_main_queue(), ^{ completion(NO); }); }
  });
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center willPresentNotification:(UNNotification *)notification
    withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
  completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionSound);
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center didReceiveNotificationResponse:(UNNotificationResponse *)response
    withCompletionHandler:(void (^)(void))completion {
  NSDictionary *info = response.notification.request.content.userInfo;
  NSString *task = info[@"taskId"], *space = info[@"spaceId"];
  if (UUID7(task) && UUID7(space)) [self queueRoute:@{@"kind": @"task", @"id": task, @"spaceId": space}];
  completion();
}
- (void)queueRoute:(NSDictionary *)route {
  dispatch_async(dispatch_get_main_queue(), ^{
    if (self->_bridgeReady && self.view) [self.view sendGlobalEvent:@"lane.openRoute" withParams:@[route]];
    else self->_pendingRoute = route;
  });
}
- (void)completeReconnect:(NSDictionary *)credential context:(NSDictionary *)context {
  NSString *space = context[@"spaceId"];
  __block BOOL valid = NO;
  dispatch_sync(_storage, ^{
    try {
      NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
      valid = [status[@"accountId"] isEqual:context[@"accountId"]] &&
        [credential[@"identity"][@"accountId"] isEqual:context[@"accountId"]] &&
        [status[@"deviceId"] isEqual:credential[@"device_id"]] &&
        [Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue];
      if (valid) {
        NSDictionary *pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
        valid = [pending[@"epoch"] isEqual:context[@"epoch"]];
      }
    } catch (...) {}
  });
  NSDictionary *pull = valid ? [_transport callService:@"lane.v1.SyncService" method:@"PullChanges"
    request:@{@"formatVersion": @3, @"epoch": context[@"epoch"], @"cursor": context[@"cursor"], @"pageSize": @100}
    token:credential[@"access_token"]] : LaneJSON(LaneFailure(@"STALE_EPOCH", NO));
  dispatch_async(_storage, ^{
    NSString *result = LaneFailure(@"STALE_EPOCH", NO);
    try {
      if (valid && [Response(self->_store->acquireSync(UTF8(space), UTF8(self->_owner)))[@"acquired"] boolValue]) {
        if (!pull[@"error"]) {
          // A matching ACCOUNT deletion wins, including over edits made during
          // approval. The engine checks the pinned registry account atomically.
          BOOL accountDeletion = NO;
          if ([pull[@"changes"] isKindOfClass:NSArray.class]) for (id change in pull[@"changes"])
            if ([change isKindOfClass:NSDictionary.class] && [change[@"deletion"] isKindOfClass:NSDictionary.class] &&
                [change[@"deletion"][@"kind"] isEqual:@"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT"]) accountDeletion = YES;
          if (accountDeletion) result = String(self->_store->applySync(UTF8(space), UTF8(LaneEncode(@{
            @"formatVersion": @3, @"epoch": context[@"epoch"], @"owner": self->_owner, @"pull": pull}))));
        } else if ([pull[@"error"][@"reason"] isEqual:@"ERROR_REASON_STALE_EPOCH"]) {
          NSDictionary *status = LaneJSON(String(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}})))));
          NSDictionary *pending = Response(self->_store->pendingSync(UTF8(space), UTF8(self->_owner)));
          if ([status[@"sequence"] isEqual:context[@"sequence"]] &&
              [status[@"response"][@"accountId"] isEqual:context[@"accountId"]] &&
              [pending[@"epoch"] isEqual:context[@"epoch"]] &&
              [self->_authorizationContext[@"challengeId"] isEqual:credential[@"approvalState"]]) {
            NSDictionary *approval = LaneJSON(String(self->_store->approveAccount(UTF8(LaneEncode(credential[@"identity"])), false)));
            if ([approval[@"ok"] boolValue] && [approval[@"response"][@"spaceId"] isEqual:space] &&
                [self->_credentials save:credential space:space]) {
              result = String(self->_store->confirmSyncEpoch(UTF8(space), UTF8(credential[@"identity"][@"epoch"])));
            }
          }
        }
      }
      self->_store->acquireSync(UTF8(space), UTF8(self->_owner), 0);
    } catch (...) { result = LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES); }
    if ([self->_authorizationContext[@"challengeId"] isEqual:credential[@"approvalState"]]) self->_authorizationContext = nil;
    if ([LaneJSON(result)[@"ok"] boolValue]) {
      [self queueRoute:@{@"kind": @"open", @"spaceId": space}];
      [self poll]; [self syncWithCompletion:^(BOOL ignored) {}];
    } else {
      NSDictionary *error = LaneJSON(result);
      dispatch_async(dispatch_get_main_queue(), ^{ [self.view sendGlobalEvent:@"lane.authStatus" withParams:@[error]]; });
    }
  });
}
- (BOOL)handleURL:(NSURL *)url {
  NSString *scheme = [NSBundle.mainBundle objectForInfoDictionaryKey:@"LaneURLScheme"];
  if (![url.scheme isEqualToString:scheme] || url.user || url.password || url.port || url.fragment) return NO;
  if ([url.host isEqualToString:@"auth"] && [url.path isEqualToString:@"/callback"]) {
    NSArray<NSURLQueryItem *> *items = [NSURLComponents componentsWithURL:url resolvingAgainstBaseURL:NO].queryItems;
    NSMutableArray *challenges = [NSMutableArray new];
    for (NSURLQueryItem *item in items) if ([item.name isEqual:@"reauth_challenge"]) [challenges addObject:item.value ?: @""];
    if (challenges.count) {
      if (items.count != 1 || challenges.count != 1 || ![challenges[0] length]) return NO;
      NSString *challenge = challenges[0];
      dispatch_async(_storage, ^{
        NSDictionary *binding = [self->_reauthentication bindingForChallenge:challenge selectedSpace:self->_selectedSpace];
        NSUInteger generation = self->_selectionGeneration;
        if (!binding || self->_reauthGeneration != generation) return;
        NSString *space = binding[@"spaceId"];
        dispatch_async(self->_network, ^{
          NSString *token = [self->_credentials tokenForSpace:space];
          if (!token.length) return;
          NSDictionary *response = [self->_transport postJSON:@{@"challenge_id": challenge} path:@"/auth/reauth/proof" token:token];
          dispatch_async(self->_storage, ^{
            if (generation != self->_selectionGeneration || ![space isEqual:self->_selectedSpace]) return;
            NSDictionary *status = Response(self->_store->invoke("GetLocalStatus", UTF8(LaneEncode(@{@"spaceId": space, @"request": @{}}))));
            if (![status[@"accountId"] isEqual:binding[@"accountId"]] || ![status[@"syncEnabled"] boolValue]) return;
            if (![self->_reauthentication acceptProof:response[@"reauth_proof"] challenge:challenge selectedSpace:space]) return;
            dispatch_async(dispatch_get_main_queue(), ^{
              // No proof or account credential ever crosses the JS bridge.
              [self.view sendGlobalEvent:@"lane.reauthenticated" withParams:@[@{@"challengeId": challenge}]];
            });
          });
        });
      });
      return YES;
    }
    dispatch_async(_network, ^{
      NSDictionary *credential = [self->_credentials complete:url];
      if (!credential) {
        // Failed callbacks cannot grant reconnect permission or trigger reupload.
        dispatch_async(self->_storage, ^{ self->_authorizationContext = nil; });
        dispatch_async(dispatch_get_main_queue(), ^{ [self.view sendGlobalEvent:@"lane.authStatus" withParams:@[LaneJSON(LaneFailure(@"UNAUTHENTICATED", NO))]]; });
        return;
      }
      __block NSDictionary *context;
      dispatch_sync(self->_storage, ^{
        if ([self->_authorizationContext[@"challengeId"] isEqual:credential[@"approvalState"]]) context = [self->_authorizationContext copy];
      });
      if ([context[@"reconnect"] boolValue]) {
        [self completeReconnect:credential context:context]; return;
      }
      dispatch_async(dispatch_get_main_queue(), ^{
        UIViewController *controller = self.view.window.rootViewController;
        if (!controller) return;
        UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"Connect synchronization"
          message:@"Include this device’s standalone data in the approved account? Retained data from other accounts stays separate."
          preferredStyle:UIAlertControllerStyleAlert];
        void (^approve)(BOOL) = ^(BOOL merge) {
          dispatch_async(self->_storage, ^{
            if (![self ensureStore]) return;
            try {
              NSString *result = String(self->_store->approveAccount(UTF8(LaneEncode(credential[@"identity"])), merge));
              NSString *space = LaneJSON(result)[@"response"][@"spaceId"];
              if (UUID7(space) && [self->_credentials save:credential space:space]) {
                [self->_knownSpaces addObject:space]; [self poll];
                [self updatePushRegistration:space]; [self syncWithCompletion:^(BOOL ignored) {}];
                dispatch_async(dispatch_get_main_queue(), ^{ [self queueRoute:@{@"kind": @"open", @"spaceId": space}]; });
              } else {
                dispatch_async(dispatch_get_main_queue(), ^{ [self.view sendGlobalEvent:@"lane.authStatus" withParams:@[LaneJSON(LaneFailure(@"DEPENDENCY_UNAVAILABLE", YES))]]; });
              }
            } catch (...) {}
          });
        };
        [alert addAction:[UIAlertAction actionWithTitle:@"Keep separate" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) { approve(NO); }]];
        [alert addAction:[UIAlertAction actionWithTitle:@"Include local data" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) { approve(YES); }]];
        [alert addAction:[UIAlertAction actionWithTitle:@"Cancel" style:UIAlertActionStyleCancel handler:nil]];
        [controller presentViewController:alert animated:YES completion:nil];
      });
    }); return YES;
  }
  if (url.query.length) return NO;
  if ([url.host isEqualToString:@"open"] && (!url.path.length || [url.path isEqualToString:@"/"])) { [self queueRoute:@{@"kind": @"open"}]; return YES; }
  NSArray *segments = [url.path componentsSeparatedByString:@"/"];
  if (segments.count != 2 || !UUID7(segments[1])) return NO;
  if ([url.host isEqualToString:@"task"]) { [self queueRoute:@{@"kind": @"task", @"id": segments[1]}]; return YES; }
  if ([url.host isEqualToString:@"repository"]) { [self queueRoute:@{@"kind": @"repository", @"id": segments[1]}]; return YES; }
  return NO;
}
@end
