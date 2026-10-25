// SPDX-License-Identifier: Apache-2.0
#import "LaneDeletion.h"
LaneDeletionDisposition LaneValidateDeletion(NSDictionary *directive, NSString *account, NSString *device, NSString *epoch) {
  if (!account.length || !device.length || !epoch.length || directive[@"error"] ||
      ![directive[@"disconnected"] isKindOfClass:NSNumber.class] || ![directive[@"disconnected"] boolValue] ||
      ![directive[@"account_id"] isEqual:account] || ![directive[@"device_id"] isEqual:device] ||
      ![directive[@"old_epoch"] isEqual:epoch] || ![directive[@"new_epoch"] isKindOfClass:NSString.class] ||
      ![directive[@"new_epoch"] length] || [directive[@"new_epoch"] isEqual:epoch]) return LaneDeletionInvalid;
  if ([directive[@"mode"] isEqual:@"DATA_DELETION_MODE_DELETE_EVERYWHERE"]) return LaneDeletionErase;
  if ([directive[@"mode"] isEqual:@"DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"]) return LaneDeletionRetain;
  return LaneDeletionInvalid;
}
