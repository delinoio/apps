---
"@apps/lane-chrome-extension": none
---

Remove top-level await from the extension service worker so Chromium can start it.
Register all listeners synchronously and make credential access, native messaging,
screenshots and selector injection wait for trusted-context storage initialization,
failing closed with a reload remedy when it is rejected. Chrome load and worker
restart acceptance is not covered by the Node fixtures.
