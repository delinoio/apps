# RealQA for Lane privacy

RealQA for Lane helps you capture a visible page element and prepare a QA task in the Lane desktop app. Captures occur when you explicitly select an element. The extension does not collect browsing history in the background, scroll pages automatically, or capture incognito pages.

## Information used

For a capture, the extension uses pixels from the visible tab to create a cropped PNG of the selected element. The full viewport is processed only in memory and is not saved. The cropped PNG, current page URL, page title, and CSS selector are sent to the running Lane app on your device. URLs can contain private query values. Inspect, edit, or remove the metadata and cover private image content before saving in Lane.

The extension stores a random profile identifier, the profile name you enter, and a local connection credential in Chrome's local extension storage. It uses these values to identify a connection you approved in Lane. Temporary connection messages use session storage. The extension does not store images or page content in extension storage.

## Where information goes

The extension sends captures through Chrome Native Messaging to the approved Lane desktop installation on your device. It has no remote upload endpoint, analytics, advertising, or tracking service. It does not sell your data or use it for advertising or credit decisions. Use and transfer of information are limited to providing the user-requested capture workflow.

You decide whether to save the draft as a Lane task. Lane retains the final image and the metadata you keep with that task. If you enable Lane account synchronization, saved tasks and attachments can be uploaded through Lane and become available on your authorized devices and clients. Local capture does not require an account. Unsaved drafts remain in Lane memory until you discard them or quit.

## Your controls

Cancel element selection with Esc. Edit or remove browser metadata before saving. Disconnect the profile in Lane or in the extension to invalidate its credential. Remove the extension to remove its Chrome-local storage. Deleting the owning task or repository in Lane removes its saved attachments under Lane's deletion and synchronization controls. Disconnecting the extension alone does not delete saved Lane tasks.

## Contact

Use the publisher support contact shown on the official Chrome Web Store listing. A publisher must provide that contact and host this policy at its reviewed public URL before publication.
