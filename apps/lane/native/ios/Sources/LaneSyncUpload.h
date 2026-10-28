// SPDX-License-Identifier: Apache-2.0
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN
typedef NSArray<NSDictionary *> * _Nullable (^LanePendingMutations)(void);
typedef NSDictionary * _Nonnull (^LanePushMutations)(NSArray<NSDictionary *> *mutations);
typedef BOOL (^LaneApplyPush)(NSDictionary *response);

// Returns nil after bounded progress, or a sync error reason. Callers pull even
// when more eligible uploads remain for the next synchronization opportunity.
NSString * _Nullable LaneUploadPending(NSDictionary *pending, NSDictionary *info,
    LanePendingMutations reload, LanePushMutations push, LaneApplyPush apply);
NS_ASSUME_NONNULL_END
