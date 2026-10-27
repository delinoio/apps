// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
#import "LaneHTTPSImageData.h"
#include <assert.h>
#include <atomic>

typedef NS_ENUM(NSInteger, LaneFixture) { LaneFixturePNG, LaneFixtureHTTP, LaneFixtureInvalid,
    LaneFixtureError, LaneFixtureDelay, LaneFixtureRedirect };
static std::atomic<LaneFixture> fixture;
static std::atomic<NSUInteger> requests;
static std::atomic<NSUInteger> httpRequests;
static std::atomic<NSUInteger> callbacks;
static NSData *png;

@interface LaneImageProtocol : NSURLProtocol
@end
@implementation LaneImageProtocol
+ (BOOL)canInitWithRequest:(NSURLRequest *)request { return YES; }
+ (NSURLRequest *)canonicalRequestForRequest:(NSURLRequest *)request { return request; }
- (void)startLoading {
  requests++;
  assert(self.request.cachePolicy == NSURLRequestReloadIgnoringLocalCacheData);
  if ([self.request.URL.scheme isEqualToString:@"http"]) { httpRequests++; return; }
  if (fixture == LaneFixtureDelay) return;
  if (fixture == LaneFixtureRedirect) {
    NSURL *destination = [NSURL URLWithString:@"http://fixture.test/unsafe.png"];
    NSHTTPURLResponse *redirect = [[NSHTTPURLResponse alloc] initWithURL:self.request.URL statusCode:302
        HTTPVersion:@"HTTP/1.1" headerFields:@{@"Location": destination.absoluteString}];
    [self.client URLProtocol:self wasRedirectedToRequest:[NSURLRequest requestWithURL:destination]
        redirectResponse:redirect];
    [self.client URLProtocolDidFinishLoading:self];
    return;
  }
  if (fixture == LaneFixtureError) {
    [self.client URLProtocol:self didFailWithError:[NSError errorWithDomain:NSURLErrorDomain
        code:NSURLErrorSecureConnectionFailed userInfo:nil]];
    return;
  }
  NSInteger status = fixture == LaneFixtureHTTP ? 404 : 200;
  NSHTTPURLResponse *response = [[NSHTTPURLResponse alloc] initWithURL:self.request.URL statusCode:status
      HTTPVersion:@"HTTP/1.1" headerFields:@{@"Content-Type": @"image/png"}];
  [self.client URLProtocol:self didReceiveResponse:response cacheStoragePolicy:NSURLCacheStorageNotAllowed];
  [self.client URLProtocol:self didLoadData:fixture == LaneFixtureInvalid ? [@"invalid" dataUsingEncoding:NSUTF8StringEncoding] : png];
  [self.client URLProtocolDidFinishLoading:self];
}
- (void)stopLoading {}
@end

static void check(LaneHTTPSImageData *images, LaneFixture scenario, BOOL succeeds) {
  fixture = scenario;
  requests = 0;
  httpRequests = 0;
  callbacks = 0;
  dispatch_semaphore_t finished = dispatch_semaphore_create(0);
  [images loadURL:[NSURL URLWithString:@"https://fixture.test/image.png"] completion:^(NSData *data, NSError *error) {
    callbacks++;
    assert((data != nil) == succeeds);
    assert((error == nil) == succeeds);
    dispatch_semaphore_signal(finished);
  }];
  assert(dispatch_semaphore_wait(finished, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
  assert(requests.load() == 1);
  assert(httpRequests.load() == 0);
  assert(callbacks.load() == 1);
}

int main() {
  @autoreleasepool {
    png = [[NSData alloc] initWithBase64EncodedString:
        @"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/RZkAAAAASUVORK5CYII=" options:0];
    assert([LaneHTTPSImageData isPNG:png]);
    for (NSString *source in @[@"http://fixture.test/image.png", @"data:image/png;base64,x", @"https://user:pass@fixture.test/image.png"])
      assert(![LaneHTTPSImageData allowsURL:[NSURL URLWithString:source]]);
    assert([LaneHTTPSImageData allowsURL:[NSURL URLWithString:@"https://fixture.test/image.png"]]);
    NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    config.protocolClasses = @[LaneImageProtocol.class];
    LaneHTTPSImageData *images = [[LaneHTTPSImageData alloc] initWithConfiguration:config];
    check(images, LaneFixturePNG, YES);
    check(images, LaneFixtureHTTP, NO);
    check(images, LaneFixtureInvalid, NO);
    check(images, LaneFixtureError, NO);
    check(images, LaneFixtureRedirect, NO);
    fixture = LaneFixtureDelay;
    requests = 0;
    callbacks = 0;
    dispatch_block_t cancel = [images loadURL:[NSURL URLWithString:@"https://fixture.test/delayed.png"]
        completion:^(NSData *data, NSError *error) { callbacks++; }];
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:5];
    while (requests.load() == 0 && deadline.timeIntervalSinceNow > 0) [NSThread sleepForTimeInterval:0.01];
    assert(requests.load() == 1);
    cancel();
    [NSThread sleepForTimeInterval:0.2];
    assert(callbacks.load() == 0);
  }
  return 0;
}
