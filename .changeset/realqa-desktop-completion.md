---
"@apps/lane": patch
"@apps/lane-chrome-extension": patch
---

Complete remaining RealQA desktop and settings behavior. Report Chrome bridge start failures as an error state with retry or explicit approval reset, cancel pending approvals when their connection closes, report post-selection Chrome failures to Lane, reserve Edit-menu shortcuts, complete the macOS key map, activate Lane for region selection, mark clipped DOM captures and keep empty metadata values. Render host status identifiers as English text, expose selected state and validation to assistive technology, route Escape to the topmost dialog, return dialog focus, confirm discarding shortcut edits and offer Retry capture. The build accepts an optional published extension ID for the Install extension listing.

<!-- public-changelog:start -->

RealQA explains Chrome connection problems and offers a way to recover, and Chrome capture failures now show specific guidance in Lane. Capture shortcuts can no longer replace copy, paste or other editing shortcuts, and every letter key can be assigned on macOS. Region selection responds to the keyboard immediately when started from a shortcut. A failed capture offers Retry capture, partially visible Chrome elements show how to capture more, and screen reader and keyboard support is improved across RealQA settings and the capture composer.
<!-- public-changelog:end -->
