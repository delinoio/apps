# FixKey

FixKey corrects words typed with the wrong keyboard layout. It supports Korean Dubeolsik and US English QWERTY in both directions. FixKey runs locally on your desktop and requires no account.

## Install FixKey

Open [FixKey releases](https://github.com/delinoio/apps/releases?q=fixkey%40) and choose the package for your operating system and processor:

- macOS ARM64: DMG for Apple silicon. Move FixKey to Applications and open it.
- Windows x64 or ARM64: EXE installer for your processor. Windows packages do not have an Authenticode publisher signature, so Windows may show an unknown-publisher or SmartScreen warning.
- Linux X11 x64 or ARM64: AppImage for your processor. Wayland is unsupported.

Minimum runtime requirements are macOS 11 on Apple silicon; Windows 10 with WebView2; or Linux with X11, GTK 3, WebKitGTK 4.1 and an AT-SPI accessibility bus. These are platform prerequisites. They do not establish compatibility with every application or text field.

## Grant permissions

FixKey needs your operating system's accessibility and input permissions to read and replace the current word in an editable field. Follow the permission dialog, grant the requested access in system settings, then choose **Recheck**. Correction stays inactive until the required permissions and bundled dictionaries are ready. Password and protected fields are excluded.

On macOS, check **Accessibility** and **Input Monitoring** in System Settings. Accessibility can grant effective input access without a separate Input Monitoring entry. To check that list, open **General** in FixKey and choose **Set up Input Monitoring**. If FixKey is absent, use **+** in the system permission list to add the current FixKey.app from Applications, then return to FixKey and recheck. A separate list entry is not required when macOS already grants effective input access.

## Correct a word

Both layouts are enabled by default. Type in an editable field and finish a word with a space or punctuation. FixKey checks candidates for the enabled layouts against its bundled dictionaries. It replaces the word only when exactly one enabled language recognizes its candidate and that candidate differs from the original. If no language or more than one language recognizes a candidate, the word stays unchanged.

For example, the key sequences `rPthrgo` and `zlqhemrk` can become `계속해` and `키보드가`. These examples illustrate layout correction; results depend on the current field, input context and dictionary recognition.

FixKey preserves the separator after a corrected word. Enter and Tab do not trigger correction. Punctuation used in URLs or email addresses can delay evaluation until FixKey can classify the whole token. Apostrophes and hyphens between letters stay inside the word.

Automatic correction skips pasted or programmatically inserted text, numbers, emoji, URLs, email addresses and mixed-script tokens. It waits for Korean input composition to finish. Unknown fields, unavailable permissions and fields that cannot be safely changed remain untouched.

After a successful automatic correction on macOS or Windows, FixKey also tries to select an enabled input source for the corrected language. On Windows, it can use the Microsoft Korean IME's Hangul and Latin modes, including when no separate US QWERTY source is enabled. Keep your Korean IME configured for Dubeolsik. If input-source selection is unavailable or fails, the corrected text and immediate undo remain available. Closed or unsupported Windows IMEs are not changed. Linux keeps the current input source. Immediate undo does not change the input source again.

### Undo a correction

Press Backspace immediately after an automatic correction to restore the original word and its separator. Further Backspace presses delete normally. Moving the caret, changing the selection, switching fields or starting another word ends this immediate undo context.

### Choose a candidate manually

1. Open **Shortcuts** and assign the manual-candidate shortcut. It is unassigned by default. If registration fails, check the displayed reason or choose another shortcut.
2. Type a word in an editable field and use the shortcut.
3. Use the arrow keys to select a candidate. Press Enter to apply it or Esc to close the picker.

The picker shows dictionary recognition results. You can explicitly choose an unrecognized candidate. Moving the source caret or switching applications closes the picker. The picker is unavailable when FixKey cannot establish a safe replacement target.

## Configure correction

In **General**, pause or resume correction, select enabled languages, enable login startup, and choose the interface language and theme. Login startup is off by default. Settings apply immediately and are saved locally.

To keep an application unchanged, add it under **Excluded Apps**. Exclusions identify the application rather than just its displayed name. An unresolved application remains listed so you can review it.

## Privacy and offline use

FixKey processes input on your device. It never saves or transmits input text, candidates, document content, window titles or correction history. It does not use the clipboard for replacement. There is no telemetry, remote crash reporting or automatic diagnostic upload.

Dictionaries are bundled, so correction works offline from first launch once permissions are ready. Dictionaries change with application updates; there is no separate dictionary download or user dictionary editor. Network use is limited to update checks and downloads.

## Update FixKey

FixKey checks for stable updates at startup, every 24 hours while running, and when you request a check in **Updates**. Download and installation require your consent. Update payload signatures are verified, and invalid signatures prevent installation. Correction stops before installation, and settings are preserved.

After a failed check, retry manually or wait for the next scheduled check. If installation fails, restart FixKey or reinstall the appropriate package from FixKey releases. You can manually reinstall a previous release; FixKey never downgrades automatically.

## Troubleshooting and support

If correction is inactive, open **Diagnostics and Help** and read the status and reason code. Recheck permissions and try an editable field in another application. A waiting-for-an-editable-field status means the current field could not be identified safely. Also check whether correction is paused or the application is excluded.

Use **Open log folder** to inspect local diagnostic logs or **Delete logs** to remove them. Logs contain diagnostic outcomes and reason codes rather than input content. FixKey retains at most seven days of logs and at most 10 MiB in total.

For help, use the GitHub Issues link in **Diagnostics and Help** or open [GitHub Issues](https://github.com/delinoio/apps/issues). Include the FixKey version, operating system, affected application and reason code. Do not include private text or credentials.
