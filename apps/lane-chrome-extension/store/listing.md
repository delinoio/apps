# Chrome Web Store submission copy

Name: RealQA for Lane

Short description: Capture a visible page element into the running Lane app for human-led visual QA.

Category: Developer Tools

Language: English

## Detailed description

Turn a visible website problem into a Lane task. Select an element in Chrome, review its screenshot and browser metadata in Lane, annotate the image, and save it to the repository you choose.

- Capture only the visible portion of the element. Scroll yourself and capture again when needed.
- Review, edit, or remove the page URL, page title, and CSS selector before saving.
- Connect each Chrome profile only after approval in Lane. Disconnect it at any time.
- Work locally without an account. Lane synchronization is optional.

Requires the matching Lane desktop installation on macOS or Windows and an already running Lane app. The extension does not start Lane automatically. Chrome incognito, internal pages, the Chrome Web Store, local files, and other browsers are not supported. No automatic scrolling, stitched screenshots, AI execution, or website interaction is included.

## Single purpose

Capture a user-selected visible page element and send its PNG and editable browser metadata to the locally running Lane app for a new QA task.

## Permission justifications

- `activeTab`: temporary access to the page explicitly selected by the user, for element selection and visible-tab capture.
- `scripting`: inject the isolated element selector after user initiation. It highlights elements and captures the selected rectangle, not page HTML.
- `nativeMessaging`: send the selected capture to the matching approved Lane desktop host without an HTTP listener.
- `storage`: retain the device-local profile identifier, user-entered profile label, and connection credential. Temporary error status uses session storage. Images and browsing history are not stored here.

No broad host permissions, externally connectable web origins, remote executable code, analytics, advertising, or tracking are used.

## Data disclosure worksheet

Disclose website content (the selected visible pixels, page title, and CSS selector), web history category where applicable to the selected page URL, and authentication information (the local connection token). Collection is user-initiated and limited to the feature. Do not mark these categories as absent merely because processing is local.

Images and browser metadata go to the user's local Lane app. They are saved only when the user saves the task. Optional Lane account synchronization can upload saved tasks and images under Lane's account controls. The extension itself has no remote upload endpoint. No data is sold, used for advertising, or used for credit decisions. Data use is limited to the extension's stated purpose.

## Reviewer steps

1. Install the matching Lane review build; register the submitted extension's exact ID through that build's installer.
2. Start Lane. Open a normal Chrome profile and a public HTTP or HTTPS test page.
3. Open RealQA for Lane, enter a profile label, and connect. Reject once in Lane and confirm no capture is accepted. Connect again and approve.
4. Select an element. Confirm only its visible area reaches the Lane composer. Inspect and remove metadata before saving a local task.
5. Add a second capture. Cancel a third selection with Esc; confirm the draft is preserved.
6. Revoke the connection in Lane. Confirm an old connection cannot capture. Reapprove explicitly.
7. Quit Lane and retry. Confirm the extension explains that Lane must be started and does not launch it.
8. Verify incognito is unavailable and internal/Web Store pages give guidance without a region fallback.

No server account is required for these steps.

## Submission inputs that require the publisher

The authenticated Chrome Web Store developer account, actual assigned item/extension ID, publisher contact/support details, a published privacy-policy URL, reviewer access to the matching native build, and real screenshots of the reviewed extension/native flow are required before submission. Use `privacy.md` as the policy text and the existing approved RealQA icon bundled by the build. Capture truthful 1280×800 or 640×400 store screenshots; do not present mockups as runtime evidence. Upload the generated extension ZIP after native registration uses the assigned ID. Publication requires Google's review; an archive or submitted draft is not a published listing.
