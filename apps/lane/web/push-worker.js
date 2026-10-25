// SPDX-License-Identifier: Apache-2.0
// Push is notification delivery only. Never cache task data or queue mutations.
self.addEventListener("push", (event) => {
  let data;
  try {
    data = event.data?.json();
  } catch {
    return;
  }
  if (
    !data?.proposalId ||
    !data?.accountId ||
    typeof data.title !== "string" ||
    typeof data.body !== "string"
  )
    return;
  // The server applies the registered device's generic/detail preference.
  event.waitUntil(
    self.registration.showNotification(data.title, {
      body: data.body,
      tag: data.tag || `lane-${data.accountId}-${data.proposalId}`,
      data: { taskId: data.proposalId, accountId: data.accountId },
    }),
  );
});
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(
    clients.openWindow(
      `/?task=${encodeURIComponent(event.notification.data.taskId)}&account=${encodeURIComponent(event.notification.data.accountId)}`,
    ),
  );
});
