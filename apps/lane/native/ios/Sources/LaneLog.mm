// SPDX-License-Identifier: Apache-2.0
#import "LaneLog.h"
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>

void LaneLog(NSString *event) {
  static dispatch_queue_t queue;
  static dispatch_once_t once;
  dispatch_once(&once, ^{ queue = dispatch_queue_create("io.delino.lane.logs", DISPATCH_QUEUE_SERIAL); });
  dispatch_async(queue, ^{
    NSURL *support = [NSFileManager.defaultManager URLForDirectory:NSApplicationSupportDirectory inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
    if (!support) return;
    NSURL *root = [support URLByAppendingPathComponent:@"io.delino.lane/logs" isDirectory:YES];
    if (![NSFileManager.defaultManager createDirectoryAtURL:root withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700} error:nil]) return;
    NSString *lockPath = [root.path stringByAppendingPathComponent:@".rotation.lock"];
    int lock = open(lockPath.fileSystemRepresentation, O_WRONLY | O_CREAT | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (lock < 0) return;
    if (flock(lock, LOCK_EX) != 0) { close(lock); return; }
    NSDate *now = NSDate.date;
    NSISO8601DateFormatter *formatter = [NSISO8601DateFormatter new];
    NSString *timestamp = [formatter stringFromDate:now];
    NSString *path = [root.path stringByAppendingPathComponent:[NSString stringWithFormat:@"ios-%@.jsonl", [timestamp substringToIndex:10]]];
    NSData *line = [NSJSONSerialization dataWithJSONObject:@{@"event": event, @"platform": @"ios", @"time": timestamp} options:0 error:nil];
    int fd = open(path.fileSystemRepresentation, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd >= 0 && line) {
      NSMutableData *record = [line mutableCopy]; [record appendBytes:"\n" length:1];
      const char *bytes = (const char *)record.bytes; size_t left = record.length;
      while (left) { ssize_t count = write(fd, bytes, left); if (count <= 0) break; bytes += count; left -= count; }
    }
    if (fd >= 0) close(fd);
    NSMutableArray *files = [NSMutableArray new]; uint64_t total = 0;
    for (NSString *name in [NSFileManager.defaultManager contentsOfDirectoryAtPath:root.path error:nil]) {
      if ([name isEqualToString:@".rotation.lock"]) continue;
      NSString *candidate = [root.path stringByAppendingPathComponent:name]; struct stat info;
      if (lstat(candidate.fileSystemRepresentation, &info) != 0 || !S_ISREG(info.st_mode)) continue;
      if (info.st_mtime < now.timeIntervalSince1970 - 30 * 86400) { unlink(candidate.fileSystemRepresentation); continue; }
      total += info.st_size;
      [files addObject:@{@"path": candidate, @"time": @(info.st_mtime), @"size": @(info.st_size)}];
    }
    [files sortUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) { return [a[@"time"] compare:b[@"time"]]; }];
    for (NSDictionary *file in files) {
      if (total <= 100ULL * 1024 * 1024) break;
      if (unlink([file[@"path"] fileSystemRepresentation]) == 0) total -= [file[@"size"] unsignedLongLongValue];
    }
    flock(lock, LOCK_UN); close(lock);
  });
}
