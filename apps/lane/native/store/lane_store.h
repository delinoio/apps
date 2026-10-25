// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <functional>
#include <memory>
#include <string>

namespace lane {
// All calls, including host callbacks, run on the host's serial storage worker.
// Callbacks must marshal OS UI work themselves. Never pass credentials to JS.
struct HostHooks {
  // Returns a LaneService response object as ProtoJSON; throws on transport failure.
  std::function<std::string(std::string spaceId, std::string method,
                            std::string requestJson)> remote;
  // Returns {authorizationUrl,challengeId}; host owns PKCE and credential storage.
  std::function<std::string(std::string accountId)> authorize;
  // Returns NOTIFICATION_PERMISSION_*; invoked only on explicit enablement.
  std::function<std::string()> requestNotificationPermission;
  // Revokes/removes native credentials and stops the space's sync worker.
  std::function<void(std::string spaceId)> disconnect;
};
class Store {
 public:
  explicit Store(std::string root);
  ~Store();
  Store(const Store&) = delete;
  Store& operator=(const Store&) = delete;
  std::string open(std::string requestJson);
  std::string invoke(std::string method, std::string requestJson);
  std::string close(std::string spaceId);
  std::string poll();
  void setHostHooks(HostHooks hooks);

  // Native-only extensions. These must never be exported as arbitrary JS methods.
  // identity: {issuer,accountId,tenantId,epoch}; host must validate browser approval.
  // mergeStandalone is the user's explicit initial connection approval.
  std::string approveAccount(std::string identityJson, bool mergeStandalone);
  // Owner is a UUID v7. Acquire/renew before network work; never hold SQL locks
  // across network calls. A false acquired result leaves the other owner intact.
  std::string acquireSync(std::string spaceId, std::string owner, int seconds = 60);
  std::string pendingSync(std::string spaceId, std::string owner);
  // Applies a PushChangesResponse and/or PullChangesResponse atomically.
  // Input: {formatVersion:1,epoch,owner,push?:{...},pull?:{...}}.
  std::string applySync(std::string spaceId, std::string responseJson);
  // Pending local MCP proposals, filtered again at dispatch time. Host delivers
  // with proposalId/deviceId as the OS dedupe identity, then acknowledges.
  std::string pendingNotifications(std::string spaceId);
  std::string acknowledgeNotification(std::string spaceId, std::string proposalId);
  static std::string sqliteSourceId();
 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};
}  // namespace lane
