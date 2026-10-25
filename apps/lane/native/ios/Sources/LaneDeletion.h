// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>
// A deletion-status directive is authority only for its exact persisted binding.
typedef NS_ENUM(NSInteger, LaneDeletionDisposition) {
  LaneDeletionInvalid, LaneDeletionErase, LaneDeletionRetain
};
LaneDeletionDisposition LaneValidateDeletion(NSDictionary *directive, NSString *account, NSString *device, NSString *epoch);
