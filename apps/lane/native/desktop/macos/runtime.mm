// SPDX-License-Identifier: Apache-2.0
#import "runtime.h"
#include "log.h"
#include "../../store/lane_store.h"
#include <chrono>
#include <set>
#include <stdexcept>

namespace {
struct HelperFailure : std::runtime_error {
  std::string envelope;
  explicit HelperFailure(std::string value) : std::runtime_error("Helper rejected operation"), envelope(std::move(value)) {}
};
std::string JSON(id value) {
  NSData* data = [NSJSONSerialization dataWithJSONObject:value options:NSJSONWritingSortedKeys error:nil];
  if (!data) throw std::runtime_error("Invalid native response");
  return std::string(static_cast<const char*>(data.bytes), data.length);
}
id Parse(const std::string& value) {
  return [NSJSONSerialization JSONObjectWithData:[NSData dataWithBytes:value.data() length:value.size()]
    options:0 error:nil];
}
NSString* Text(const std::string& value) { return [[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding]; }
std::string UUID() {
  uint8_t bytes[16]; arc4random_buf(bytes, sizeof(bytes));
  auto millis = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::system_clock::now().time_since_epoch()).count();
  for (int i=5; i>=0; --i) { bytes[i] = millis & 255; millis >>= 8; }
  bytes[6] = (bytes[6] & 15) | 0x70; bytes[8] = (bytes[8] & 63) | 0x80;
  return [[[NSUUID alloc] initWithUUIDBytes:bytes].UUIDString lowercaseString].UTF8String;
}
bool IsID(NSString* value) {
  if (![value isKindOfClass:NSString.class] || value.length != 36) return false;
  NSRegularExpression* pattern = [NSRegularExpression regularExpressionWithPattern:@"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$" options:0 error:nil];
  return [pattern numberOfMatchesInString:value options:0 range:NSMakeRange(0,value.length)] == 1;
}
NSString* Scheme() { return [[NSBundle mainBundle] objectForInfoDictionaryKey:@"LaneURLScheme"] ?: @"lane-dev"; }
std::string Failure(NSString* reason, NSString* message) {
  return JSON(@{@"ok":@NO,@"error":@{@"reason":reason,@"message":message,@"correlationId":Text(UUID()),@"retryable":@NO}});
}
}

@implementation LaneRuntime {
  dispatch_queue_t _queue;
  dispatch_source_t _timer;
  std::unique_ptr<lane::Store> _store;
  NSMutableDictionary<NSString*, NSTask*>* _workers;
  NSMutableDictionary<NSString*, NSDate*>* _retryAfter;
  NSMutableDictionary<NSString*, NSString*>* _workerStates;
  NSMutableSet<NSString*>* _spaces;
  NSMutableArray<NSDictionary*>* _routes;
  NSString* _currentSpace;
  NSString* _authorizationSpace;
  std::string _claimant;
  BOOL _stopped;
}
- (instancetype)init {
  if ((self = [super init])) {
    _queue = dispatch_queue_create("io.delino.lane.storage", DISPATCH_QUEUE_SERIAL);
    _workers = [NSMutableDictionary new]; _retryAfter = [NSMutableDictionary new]; _workerStates = [NSMutableDictionary new];
    _spaces = [NSMutableSet new]; _routes = [NSMutableArray new]; _claimant = UUID();
    _timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, _queue);
    dispatch_source_set_timer(_timer, dispatch_time(DISPATCH_TIME_NOW, 0), 500*NSEC_PER_MSEC, 50*NSEC_PER_MSEC);
    __weak LaneRuntime* weak = self;
    dispatch_source_set_event_handler(_timer, ^{ [weak tick]; });
    dispatch_resume(_timer);
  }
  return self;
}
- (void)ensureStore {
  if (_store) return;
  NSURL* support = [[NSFileManager defaultManager] URLForDirectory:NSApplicationSupportDirectory
    inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
  if (!support) throw std::runtime_error("Application storage unavailable");
  NSURL* root = [support URLByAppendingPathComponent:@"io.delino.lane" isDirectory:YES];
#ifdef LANE_NATIVE_TESTING
  NSString* injected = NSProcessInfo.processInfo.environment[@"LANE_NATIVE_TEST_ROOT"];
  if (injected) {
    if (!injected.isAbsolutePath) throw std::runtime_error("Test root must be absolute");
    root = [NSURL fileURLWithPath:injected isDirectory:YES];
  }
#endif
  _store = std::make_unique<lane::Store>(root.fileSystemRepresentation);
  __weak LaneRuntime* weak = self;
  lane::HostHooks hooks;
  hooks.requestNotificationPermission = [weak]() -> std::string { return [weak requestPermission]; };
  hooks.authorize = [weak](std::string account) -> std::string {
    LaneRuntime* host = weak;
    if (!host) throw std::runtime_error("Host stopped");
    // The helper owns OAuth state, PKCE and Keychain credentials. The callback
    // URI never supplies account identity to the database or JavaScript.
    NSMutableArray* args = [@[@"--json", @"--space", host->_currentSpace, @"sync", @"connect",
      @"--redirect-uri", [NSString stringWithFormat:@"%@://auth/callback", Scheme()]] mutableCopy];
    if (!account.empty()) [args addObjectsFromArray:@[@"--account-id", Text(account)]];
    NSDictionary* status = Parse(host->_store->invoke("GetLocalStatus", JSON(@{@"spaceId":host->_currentSpace,@"request":@{}})))[@"response"];
    if ([status[@"reconnectRequired"] boolValue]) {
      // The existing bridge contract uses an explicitly targeted ConnectSync
      // only after the UI confirms retained-data upload. An untargeted login
      // must never be upgraded into epoch-change approval.
      if (account.empty() || ![status[@"accountId"] isEqualToString:Text(account)])
        return Failure(@"ERROR_REASON_STALE_EPOCH", @"Confirm reconnecting this account before uploading retained data.");
      [args addObject:@"--confirm-epoch-change"];
    }
    NSDictionary* result;
    try { result = [host helper:args input:nil]; }
    catch (const HelperFailure& error) { return error.envelope; }
    NSURL* url = [NSURL URLWithString:result[@"authorizationUrl"]];
    if (![url.scheme isEqualToString:@"https"] || !url.host.length) throw std::runtime_error("Invalid authorization URL");
    host->_authorizationSpace = host->_currentSpace;
    dispatch_async(dispatch_get_main_queue(), ^{ [[NSWorkspace sharedWorkspace] openURL:url]; });
    return JSON(result);
  };
  hooks.remote = [weak](std::string space, std::string method, std::string request) -> std::string {
    try { return JSON([weak helper:@[@"--remote", @"--json", @"--expected-space", Text(space), @"rpc", Text(method), @"--request-json", @"-"] input:Text(request)]); }
    catch (const HelperFailure& error) { return error.envelope; }
  };
  hooks.disconnect = [weak](std::string space) {
    LaneRuntime* host = weak;
    [host stopWorker:Text(space)];
    NSDictionary* status = Parse(host->_store->invoke("GetLocalStatus",JSON(@{@"spaceId":Text(space),@"request":@{}})))[@"response"];
    if (![status[@"syncEnabled"] boolValue]) return;
    [host helper:@[@"--json", @"--space", Text(space), @"sync", @"disconnect", @"--retention", @"keep-local"] input:nil];
  };
  _store->setHostHooks(std::move(hooks));
  NSDictionary* opened = Parse(_store->open(R"({"formatVersion":1,"spaceId":""})"));
  if ([opened[@"ok"] boolValue]) {
    _currentSpace = opened[@"response"][@"spaceId"];
    [_spaces addObject:_currentSpace];
  }
}
- (NSURL*)helperURL {
  NSURL* url = [[[NSBundle mainBundle].executableURL URLByDeletingLastPathComponent] URLByAppendingPathComponent:@"helpers/lane"];
  if (!url || ![[NSFileManager defaultManager] isExecutableFileAtPath:url.path])
    throw std::runtime_error("Bundled synchronization helper unavailable");
  return url;
}
- (NSDictionary*)helper:(NSArray<NSString*>*)arguments input:(NSString*)input {
  NSTask* task = [NSTask new]; task.executableURL = [self helperURL]; task.arguments = arguments;
  NSPipe* output = [NSPipe pipe]; task.standardOutput = output;
  // Helper diagnostics contain metadata only, and go to the host's stderr.
  task.standardError = NSFileHandle.fileHandleWithStandardError;
  NSPipe* incoming = nil;
  if (input) { incoming = [NSPipe pipe]; task.standardInput = incoming; }
  NSError* error = nil;
  if (![task launchAndReturnError:&error]) throw std::runtime_error("Helper launch failed");
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 60*NSEC_PER_SEC), dispatch_get_global_queue(QOS_CLASS_UTILITY,0), ^{ if (task.running) [task terminate]; });
  if (incoming) {
    [incoming.fileHandleForWriting writeData:[input dataUsingEncoding:NSUTF8StringEncoding]];
    [incoming.fileHandleForWriting closeFile];
  }
  NSData* bytes = [output.fileHandleForReading readDataToEndOfFile];
  [task waitUntilExit];
  NSDictionary* response = [NSJSONSerialization JSONObjectWithData:bytes options:0 error:nil];
  if ([response isKindOfClass:NSDictionary.class] && ![response[@"ok"] boolValue] && [response[@"error"] isKindOfClass:NSDictionary.class])
    throw HelperFailure(JSON(response));
  if (task.terminationStatus != 0 || ![response isKindOfClass:NSDictionary.class] || ![response[@"ok"] boolValue])
    throw std::runtime_error("Synchronization helper rejected operation");
  if (![response[@"result"] isKindOfClass:NSDictionary.class]) throw std::runtime_error("Invalid helper response");
  return response[@"result"];
}
- (std::string)requestPermission {
  dispatch_semaphore_t ready = dispatch_semaphore_create(0);
  __block NSString* result = @"NOTIFICATION_PERMISSION_NOT_REQUESTED";
  dispatch_async(dispatch_get_main_queue(), ^{
    [[UNUserNotificationCenter currentNotificationCenter] requestAuthorizationWithOptions:UNAuthorizationOptionAlert|UNAuthorizationOptionSound
      completionHandler:^(BOOL granted, NSError* error) {
        result = error ? @"NOTIFICATION_PERMISSION_UNSUPPORTED" : granted ? @"NOTIFICATION_PERMISSION_GRANTED" : @"NOTIFICATION_PERMISSION_DENIED";
        if (granted) dispatch_async(dispatch_get_main_queue(), ^{ [NSApp registerForRemoteNotifications]; });
        dispatch_semaphore_signal(ready);
      }];
  });
  if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 120*NSEC_PER_SEC)) != 0)
    return "NOTIFICATION_PERMISSION_NOT_REQUESTED";
  return result.UTF8String;
}
- (std::shared_ptr<lane::desktop::Handler>)storeHandler {
  LaneRuntime* host = self;
  return std::make_shared<lane::desktop::Handler>([host](const std::string& method, const std::vector<std::string>& args) {
    __block std::string response;
    dispatch_sync(host->_queue, ^{
      try {
        if (host->_stopped) throw std::runtime_error("Host stopped");
        [host ensureStore];
        if (method == "open") {
          response = host->_store->open(args.at(0));
          NSDictionary* body = Parse(response);
          if ([body[@"ok"] boolValue]) {
            host->_currentSpace = body[@"response"][@"spaceId"];
            [host->_spaces addObject:host->_currentSpace];
            [host flushRoutes];
          }
        } else if (method == "close") {
          response = host->_store->close(args.at(0));
          // Keep the application service and local notifications alive.
        } else if (method == "invoke") {
          NSDictionary* route = Parse(args.at(1));
          host->_currentSpace = route[@"spaceId"];
          response = host->_store->invoke(args.at(0), args.at(1));
        }
        [host tick];
      } catch (...) { response = Failure(@"ERROR_REASON_DEPENDENCY_UNAVAILABLE", @"Native service is unavailable. Local data is retained."); }
    });
    return response;
  });
}
- (std::shared_ptr<lane::desktop::Handler>)hostHandler {
  return std::make_shared<lane::desktop::Handler>([](const std::string& method, const std::vector<std::string>& args) {
    __block BOOL success = NO;
    NSString* value = Text(args.at(0));
    dispatch_sync(dispatch_get_main_queue(), ^{
      if (method == "copyText") {
        [NSPasteboard.generalPasteboard clearContents];
        success = [NSPasteboard.generalPasteboard setString:value forType:NSPasteboardTypeString];
      } else if (method == "openURL") {
        NSURL* url = [NSURL URLWithString:value];
        if ([url.scheme isEqualToString:@"https"] && url.host.length && !url.user && !url.password)
          success = [[NSWorkspace sharedWorkspace] openURL:url];
      }
    });
    return success ? std::string("{\"ok\":true}") : Failure(@"ERROR_REASON_INVALID_INPUT", @"The host could not complete this action.");
  });
}
- (void)emit:(NSString*)name arguments:(NSArray*)arguments {
  dispatch_async(dispatch_get_main_queue(), ^{ if (self.eventSink) self.eventSink(name, arguments); });
}
- (void)tick {
  if (_stopped) return;
  @try {
    try {
      [self ensureStore];
      NSArray* changes = Parse(_store->poll());
      for (NSDictionary* change in changes) {
        if (IsID(change[@"spaceId"])) [_spaces addObject:change[@"spaceId"]];
        [self emit:@"lane.storeChanged" arguments:@[change]];
      }
      for (NSString* space in [_spaces copy]) {
        NSDictionary* status = Parse(_store->invoke("GetLocalStatus", JSON(@{@"spaceId":space,@"request":@{}})));
        if (![status[@"ok"] boolValue]) { [self stopWorker:space]; [_spaces removeObject:space]; continue; }
        NSDictionary* value = status[@"response"];
        if ([value[@"syncEnabled"] boolValue]) [self startWorker:space]; else [self stopWorker:space];
        if ([value[@"notificationPreferences"][@"enabled"] boolValue]) [self deliverNotifications:space];
      }
    } catch (...) { LaneLog("lane_native_poll_failed"); }
  } @catch (NSException*) { LaneLog("lane_native_platform_failed"); }
}
- (void)workerState:(NSString*)space state:(NSString*)state code:(int)code {
  if ([_workerStates[space] isEqualToString:state]) return;
  _workerStates[space] = state;
  if (![state isEqualToString:@"running"] && _store)
    _store->recordSyncFailure(space.UTF8String, "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
  [self emit:@"lane.hostStatus" arguments:@[@{@"spaceId":space,@"syncState":state,@"exitCode":@(code)}]];
}
- (void)startWorker:(NSString*)space {
  if (_workers[space].running) return;
  if (_workers[space]) {
    [self workerState:space state:@"exited" code:_workers[space].terminationStatus];
    [_workers removeObjectForKey:space];
  }
  if ([_retryAfter[space] timeIntervalSinceNow] > 0) return;
  _retryAfter[space] = [NSDate dateWithTimeIntervalSinceNow:30];
  try {
    NSTask* task = [NSTask new]; task.executableURL = [self helperURL];
    task.arguments = @[@"--space",space,@"sync",@"run"];
    task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
    task.standardError = NSFileHandle.fileHandleWithStandardError;
    NSError* error = nil;
    if ([task launchAndReturnError:&error]) {
      _workers[space] = task;
      [self workerState:space state:@"running" code:0];
    } else [self workerState:space state:@"launch_failed" code:0];
  } catch (...) { [self workerState:space state:@"missing_helper" code:0]; }
}
- (void)stopWorker:(NSString*)space {
  NSTask* task = _workers[space];
  if (task.running) [task terminate];
  [_workers removeObjectForKey:space];
}
- (void)deliverNotifications:(NSString*)space {
  NSDictionary* claimed = Parse(_store->claimNotifications(space.UTF8String, _claimant));
  for (NSDictionary* proposal in claimed[@"response"][@"items"]) {
    UNMutableNotificationContent* content = [UNMutableNotificationContent new];
    BOOL generic = [proposal[@"content"] isEqualToString:@"NOTIFICATION_CONTENT_GENERIC"];
    content.title = generic ? @"Lane" : proposal[@"repositoryName"] ?: @"Lane";
    content.body = generic ? @"A new task proposal is available." : proposal[@"title"] ?: @"A new task proposal is available.";
    content.userInfo = @{@"spaceId":space,@"taskId":proposal[@"proposalId"]};
    NSString* identifier = [NSString stringWithFormat:@"%@/%@",proposal[@"deviceId"],proposal[@"proposalId"]];
    UNNotificationRequest* request = [UNNotificationRequest requestWithIdentifier:identifier content:content trigger:nil];
    [[UNUserNotificationCenter currentNotificationCenter] addNotificationRequest:request withCompletionHandler:^(NSError* error) {
      if (!error) dispatch_async(self->_queue, ^{
        if (self->_store && !self->_stopped) self->_store->acknowledgeNotification(space.UTF8String, [proposal[@"proposalId"] UTF8String]);
      });
      else LaneLog("lane_notification_delivery_failed");
    }];
  }
}
- (void)resume {
  dispatch_async(_queue, ^{
    [self tick];
    [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings* settings) {
      NSString* permission = settings.authorizationStatus == UNAuthorizationStatusNotDetermined ? @"NOTIFICATION_PERMISSION_NOT_REQUESTED" :
        settings.authorizationStatus == UNAuthorizationStatusDenied ? @"NOTIFICATION_PERMISSION_DENIED" : @"NOTIFICATION_PERMISSION_GRANTED";
      dispatch_async(self->_queue, ^{ if (!self->_stopped && self->_store) for (NSString* space in self->_spaces) self->_store->recordNotificationPermission(space.UTF8String,permission.UTF8String); });
    }];
  });
}
- (void)handleURL:(NSURL*)url {
  if (![url.scheme isEqualToString:Scheme()] || url.user || url.password || url.fragment) return;
  dispatch_async(_queue, ^{
    try {
      [self ensureStore];
      if ([url.host isEqualToString:@"auth"] && [url.path isEqualToString:@"/callback"]) {
        NSString* authorizationSpace = self->_authorizationSpace ?: self->_currentSpace;
        if (!IsID(authorizationSpace)) return;
        NSDictionary* result = [self helper:@[@"--json",@"--space",authorizationSpace,@"sync",@"callback",url.absoluteString] input:nil];
        self->_authorizationSpace = nil;
        if (IsID(result[@"spaceId"])) {
          [self emit:@"lane.storeChanged" arguments:@[@{@"spaceId":result[@"spaceId"],@"sequence":result[@"sequence"] ?: @"0"}]];
          [self->_routes addObject:@{@"kind":@"open",@"spaceId":result[@"spaceId"]}];
          [self flushRoutes];
        }
        [self tick]; return;
      }
      if (url.query.length) return;
      NSArray* components = url.path.pathComponents;
      NSDictionary* route = nil;
      if ([url.host isEqualToString:@"open"] && (url.path.length == 0 || [url.path isEqualToString:@"/"])) route = @{@"kind":@"open"};
      else if (([url.host isEqualToString:@"repository"] || [url.host isEqualToString:@"task"]) && components.count == 2 && IsID(components[1]))
        route = @{@"kind":url.host,@"id":components[1]};
      if (route) { [self->_routes addObject:route]; [self flushRoutes]; }
    } catch (...) { [self emit:@"lane.hostError" arguments:@[@{@"reason":@"ERROR_REASON_DEPENDENCY_UNAVAILABLE",@"message":@"The authorization callback could not be completed."}]]; }
  });
}
- (void)handleNotification:(NSDictionary*)info {
  NSString* space = info[@"spaceId"], *task = info[@"taskId"];
  if (!IsID(space) || !IsID(task)) return;
  dispatch_async(_queue, ^{
    try {
      [self ensureStore];
      // A notification pins its account space. Never reinterpret its task ID
      // in the active space, which may belong to a different retained account.
      NSDictionary* opened = Parse(self->_store->open(JSON(@{@"formatVersion":@1,@"spaceId":space})));
      if (![opened[@"ok"] boolValue]) return;
      self->_store->close(space.UTF8String);
      [self->_routes addObject:@{@"kind":@"task",@"id":task,@"spaceId":space}];
      [self flushRoutes];
    } catch (...) { LaneLog("lane_notification_route_failed"); }
  });
}
- (void)flushRoutes {
  if (!_currentSpace) return;
  for (NSDictionary* route in _routes) [self emit:@"lane.openRoute" arguments:@[route]];
  [_routes removeAllObjects];
}
- (void)receivePush:(NSDictionary*)payload {
  if (![payload[@"type"] isEqualToString:@"lane.proposal"] ||
      ![payload[@"accountId"] isKindOfClass:NSString.class] || !IsID(payload[@"proposalId"])) return;
  NSString* account = payload[@"accountId"];
  dispatch_async(_queue, ^{
    try {
      [self ensureStore];
      NSDictionary* matched = Parse(self->_store->spacesForAccount(account.UTF8String));
      if (![matched[@"ok"] boolValue]) return;
      for (NSString* space in matched[@"response"][@"items"]) {
        [self->_spaces addObject:space];
        [self startWorker:space];
      }
      // Push is only a wake hint. The authenticated worker downloads versions;
      // ordinary store observation later claims eligible local notifications.
    } catch (...) { LaneLog("lane_push_wake_failed"); }
  });
}
- (void)registerPushToken:(NSData*)token {
  // Register only after explicit notification enablement. Credentials remain in
  // the helper's Keychain. A missing helper/provider leaves local use available.
  NSMutableString* hex = [NSMutableString new];
  const unsigned char* bytes = static_cast<const unsigned char*>(token.bytes);
  for (NSUInteger i=0;i<token.length;++i) [hex appendFormat:@"%02x",bytes[i]];
  dispatch_async(_queue, ^{
    for (NSString* space in self->_spaces) {
      try {
        NSDictionary* status = Parse(self->_store->invoke("GetLocalStatus",JSON(@{@"spaceId":space,@"request":@{}})))[@"response"];
        if (![status[@"syncEnabled"] boolValue]) continue;
        NSDictionary* preferences = status[@"notificationPreferences"];
        NSDictionary* request = @{@"meta":@{@"requestId":Text(UUID())},@"deviceId":status[@"deviceId"],@"provider":@"PUSH_PROVIDER_APNS",@"endpoint":hex,@"enabled":preferences[@"enabled"] ?: @NO,@"content":preferences[@"content"] ?: @"NOTIFICATION_CONTENT_DETAIL"};
        [self helper:@[@"--remote",@"--json",@"--expected-space",space,@"rpc",@"RegisterPush",@"--request-json",@"-"] input:Text(JSON(request))];
      } catch (...) { LaneLog("lane_push_registration_unavailable"); }
    }
  });
}
- (void)stop { [self stopWithCompletion:nil]; }
- (void)stopWithCompletion:(dispatch_block_t)completion {
  // Do not wait on the storage queue from the main thread: an explicit permission
  // request can be awaiting a main-thread OS callback there.
  dispatch_source_cancel(_timer);
  dispatch_async(_queue, ^{
    self->_stopped = YES;
    for (NSString* space in [self->_workers.allKeys copy]) [self stopWorker:space];
    self->_store.reset();
    if (completion) dispatch_async(dispatch_get_main_queue(), completion);
  });
}
@end
