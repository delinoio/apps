// SPDX-License-Identifier: Apache-2.0
// Push is notification delivery only. Never cache task data or queue mutations.
self.addEventListener("push", (event) => {
  let data;
  try {
    data = event.data?.json();
  } catch {
    return;
  }
  if (!data?.proposalId) return;
  event.waitUntil(
    self.registration.showNotification(
      data.generic ? "New Lane proposal" : data.title || "New Lane proposal",
      {
        body: data.generic ? "Open Lane to review." : data.repositoryName || "",
        tag: `lane-proposal-${data.proposalId}`,
        data: { taskId: data.proposalId },
      },
    ),
  );
});
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(
    clients.openWindow(
      `/?task=${encodeURIComponent(event.notification.data.taskId)}`,
    ),
  );
});
