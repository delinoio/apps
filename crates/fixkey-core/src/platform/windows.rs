//! Windows UI Automation value/selection transactions and listen-only hooks.
//!
//! Only non-password Edit controls with writable `ValuePattern` and verifiable
//! `TextPattern` selections are eligible. Other controls are refused.

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use uiautomation::{
    UIAutomation, UIElement,
    patterns::{UITextPattern, UITextRange, UIValuePattern},
    types::{ControlType, TextPatternRangeEndpoint},
};
use windows::{
    Win32::{
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2},
    },
    core::Interface,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND},
    Storage::Packaging::Appx::GetApplicationUserModelId,
    System::{
        ProcessStatus::K32EnumProcesses,
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
    UI::{
        Input::{
            Ime::{IMC_SETCONVERSIONMODE, IME_CMODE_NATIVE, ImmGetDefaultIMEWnd},
            KeyboardAndMouse::{
                GetAsyncKeyState, GetKeyState, GetKeyboardLayout, GetKeyboardLayoutList, HKL,
            },
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GUITHREADINFO, GetGUIThreadInfo, GetMessageW,
            GetWindowThreadProcessId, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED,
            MSG, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW,
            SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
            WM_IME_CONTROL, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN, WM_SYSKEYDOWN,
        },
    },
};

use super::{Application, Cursor, Gate, Handler, Identity, Key, KeyEvent};
use crate::{
    Language, SkipReason,
    session::{Accessibility, FieldSensitivity, FieldSnapshot, TargetId, WriteError},
};

fn layout_matches(layout: HKL, language: Language) -> bool {
    let identifier = layout as usize & 0xffff_ffff;
    match language {
        Language::EnUsQwerty => identifier == 0x0409_0409,
        Language::KoDubeolsik => matches!(identifier, 0x0412_0412 | 0xe001_0412),
    }
}

fn preferred_layout(layouts: &[HKL], current: HKL, language: Language) -> Option<HKL> {
    let supports = |layout| layout_matches(layout, language);
    if layouts.contains(&current) && supports(current) {
        Some(current)
    } else {
        layouts
            .iter()
            .copied()
            .find(|layout| supports(*layout))
            .or_else(|| {
                (language == Language::EnUsQwerty)
                    .then(|| preferred_layout(layouts, current, Language::KoDubeolsik))
                    .flatten()
            })
    }
}

// Legacy IMM query commands have no windows-sys constants. Keep them local
// until Windows exposes typed cross-process mode queries. Only the captured
// foreground control's default IME window receives these bounded messages.
const GET_CONVERSION_MODE: usize = 1;
const GET_OPEN_STATUS: usize = 5;

fn select_ime_mode(
    language: Language,
    mut message: impl FnMut(usize, isize) -> Option<usize>,
) -> Option<()> {
    // A delivered unsupported query can return zero. Require an affirmative
    // open-status response before interpreting zero as a valid Latin mode.
    // Closed and unsupported IMEs are left untouched, never probed by a write.
    if message(GET_OPEN_STATUS, 0)? != 1 {
        return None;
    }
    let mode = message(GET_CONVERSION_MODE, 0)?;
    let desired = match language {
        Language::KoDubeolsik => mode | IME_CMODE_NATIVE as usize,
        Language::EnUsQwerty => mode & !(IME_CMODE_NATIVE as usize),
    };
    if desired != mode
        && message(
            IMC_SETCONVERSIONMODE as usize,
            isize::try_from(desired).ok()?,
        )? != 0
    {
        return None;
    }
    (message(GET_OPEN_STATUS, 0)? == 1 && message(GET_CONVERSION_MODE, 0)? == desired).then_some(())
}

fn send_input_message(window: HWND, message: u32, command: usize, value: isize) -> Option<usize> {
    let mut result = 0;
    // SAFETY: Messages carry integer values only, never borrowed pointers. A
    // hung or exiting target is refused after a finite timeout without retry.
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            message,
            command,
            value,
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            50,
            &raw mut result,
        )
    };
    (sent != 0).then_some(result)
}

pub struct Adapter {
    automation: Option<UIAutomation>,
    allowed: Option<Identity>,
    last: Option<(Vec<i32>, FieldSnapshot)>,
    cursor: Option<(TargetId, UITextRange, u64)>,
    cursor_version: u64,
    initialized: bool,
    thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Default for Adapter {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Adapter {
    fn drop(&mut self) {
        self.last = None;
        self.cursor = None;
        self.automation = None;
        if self.initialized {
            // SAFETY: All COM references are released first; the non-Send
            // adapter balances its successful initialization on this same
            // thread.
            unsafe {
                CoUninitialize();
            }
        }
    }
}
impl Adapter {
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: The reserved pointer is null and the MTA flags are valid.
        // Successful initialization is balanced on this thread.
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let automation = if initialized {
            // SAFETY: COM is initialized on this thread. CUIAutomation8
            // implements the timeout interface required below; the older
            // CUIAutomation class used by new_direct does not.
            unsafe {
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            }
            .ok()
            .map(UIAutomation::from)
        } else {
            None
        };
        let automation = automation.filter(|automation| {
            let raw: &IUIAutomation = automation.as_ref();
            // SAFETY: This is a live COM interface; both timeouts are finite
            // and bound provider calls.
            raw.cast::<IUIAutomation2>().is_ok_and(|automation| unsafe {
                // Windows rejects the previous 40 ms value with E_INVALIDARG.
                automation.SetConnectionTimeout(50).is_ok()
                    && automation.SetTransactionTimeout(50).is_ok()
            })
        });
        Self {
            automation,
            allowed: None,
            last: None,
            cursor: None,
            cursor_version: 0,
            initialized,
            thread: std::marker::PhantomData,
        }
    }

    pub fn clear(&mut self) {
        self.last = None;
        self.cursor = None;
        self.allowed = None;
    }

    /// Restricts future text reads to the application approved by the caller.
    pub fn set_application(&mut self, identity: &Identity) {
        self.allowed = Some(identity.clone());
    }

    #[must_use]
    pub const fn permission() -> bool {
        true
    }

    fn focus(&self) -> Result<UIElement, SkipReason> {
        let element = self
            .automation
            .as_ref()
            .ok_or(SkipReason::UnknownField)?
            .get_focused_element()
            .map_err(|_| SkipReason::UnknownField)?;
        if element
            .is_password()
            .map_err(|_| SkipReason::UnknownField)?
        {
            return Err(SkipReason::ProtectedField);
        }
        if element
            .get_control_type()
            .map_err(|_| SkipReason::UnknownField)?
            != ControlType::Edit
            || !element.is_enabled().map_err(|_| SkipReason::UnknownField)?
        {
            return Err(SkipReason::UnknownField);
        }
        let value = element
            .get_pattern::<UIValuePattern>()
            .map_err(|_| SkipReason::UnsupportedWrite)?;
        if value
            .is_readonly()
            .map_err(|_| SkipReason::UnsupportedWrite)?
            || element.get_pattern::<UITextPattern>().is_err()
        {
            return Err(SkipReason::UnsupportedWrite);
        }
        Ok(element)
    }

    fn input_target(&self) -> Option<(HWND, u32)> {
        let element = self.focus().ok()?;
        let native: windows::Win32::Foundation::HWND =
            element.get_native_window_handle().ok()?.into();
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(std::mem::size_of::<GUITHREADINFO>()).ok()?,
            ..GUITHREADINFO::default()
        };
        // SAFETY: The output structure has its required size. Thread zero
        // queries the foreground thread, not FixKey's background worker.
        if unsafe { GetGUIThreadInfo(0, &raw mut info) } == 0
            || info.hwndFocus.is_null()
            || info.hwndFocus != native.0
        {
            return None;
        }
        let mut process = 0;
        // SAFETY: This is the foreground focus HWND; the output pointer is live.
        let thread = unsafe { GetWindowThreadProcessId(info.hwndFocus, &raw mut process) };
        if thread == 0 || process != element.get_process_id().ok()? {
            return None;
        }
        Some((info.hwndFocus, thread))
    }

    fn input_target_matches(&mut self, expected: &FieldSnapshot, target: (HWND, u32)) -> bool {
        self.snapshot().as_ref() == Ok(expected) && self.input_target() == Some(target)
    }

    fn select_language(&mut self, expected: &FieldSnapshot, language: Language) -> Option<()> {
        let target = self.input_target()?;
        // SAFETY: Query only loaded layouts; never load or enable a layout.
        let count = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
        if !(1..=256).contains(&count) {
            return None;
        }
        let mut layouts = vec![std::ptr::null_mut(); usize::try_from(count).ok()?];
        // SAFETY: The buffer contains count writable HKL slots.
        let copied = unsafe { GetKeyboardLayoutList(count, layouts.as_mut_ptr()) };
        if copied <= 0 || copied > count {
            return None;
        }
        layouts.truncate(usize::try_from(copied).ok()?);
        // SAFETY: Query the captured foreground thread, not the adapter thread.
        let current = unsafe { GetKeyboardLayout(target.1) };
        let layout = preferred_layout(&layouts, current, language)?;
        if !self.input_target_matches(expected, target) {
            return None;
        }
        if current != layout {
            send_input_message(target.0, WM_INPUTLANGCHANGEREQUEST, 0, layout as isize)?;
        }
        // SAFETY: The captured thread ID is used only for a layout query.
        if unsafe { GetKeyboardLayout(target.1) } != layout {
            return None;
        }
        if layout_matches(layout, Language::KoDubeolsik) {
            // SAFETY: The HWND is used only to locate its default IME window.
            let ime = unsafe { ImmGetDefaultIMEWnd(target.0) };
            if ime.is_null() {
                return None;
            }
            select_ime_mode(language, |command, value| {
                if !self.input_target_matches(expected, target) {
                    return None;
                }
                // SAFETY: These are read-only queries of the captured thread
                // and focus window; refuse a changed input context.
                let context_matches = unsafe {
                    GetKeyboardLayout(target.1) == layout && ImmGetDefaultIMEWnd(target.0) == ime
                };
                if !context_matches {
                    return None;
                }
                send_input_message(ime, WM_IME_CONTROL, command, value)
            })?;
        }
        // SAFETY: Read back the selected layout on the captured thread.
        (self.input_target_matches(expected, target)
            && unsafe { GetKeyboardLayout(target.1) } == layout)
            .then_some(())
    }

    /// Gets a classified target without reading its text.
    /// # Errors
    /// Refuses unknown, protected or unsupported controls.
    pub fn metadata(&mut self) -> Result<(TargetId, Application), SkipReason> {
        let element = self.focus()?;
        if element
            .get_process_id()
            .map_err(|_| SkipReason::UnknownField)?
            == std::process::id()
        {
            return Err(SkipReason::UnknownField);
        }
        Ok((
            target(&element)?,
            application(
                element
                    .get_process_id()
                    .map_err(|_| SkipReason::UnknownField)?,
            )?,
        ))
    }

    /// Compares native selection endpoints without retrieving document text.
    /// # Errors
    /// Refuses unsafe controls, unapproved applications or unavailable ranges.
    pub fn cursor(&mut self) -> Result<Cursor, SkipReason> {
        let element = self.focus()?;
        let application = application(
            element
                .get_process_id()
                .map_err(|_| SkipReason::UnknownField)?,
        )?;
        if self.allowed.as_ref() != Some(&application.identity) {
            return Err(SkipReason::ExcludedApp);
        }
        let target = target(&element)?;
        let selections = element
            .get_pattern::<UITextPattern>()
            .map_err(|_| SkipReason::UnsupportedWrite)?
            .get_selection()
            .map_err(|_| SkipReason::UnknownField)?;
        if selections.len() != 1 {
            return Err(SkipReason::UnknownField);
        }
        let selected = selections
            .into_iter()
            .next()
            .ok_or(SkipReason::UnknownField)?;
        if let Some((previous, range, version)) = &self.cursor
            && *previous == target
            && range
                .compare(&selected)
                .map_err(|_| SkipReason::UnknownField)?
        {
            return Ok(Cursor {
                target,
                marker: [*version, 0, 0],
            });
        }
        self.cursor_version = self
            .cursor_version
            .checked_add(1)
            .ok_or(SkipReason::UnknownField)?;
        self.cursor = Some((target, selected, self.cursor_version));
        Ok(Cursor {
            target,
            marker: [self.cursor_version, 0, 0],
        })
    }

    /// Gets accessible input geometry.
    /// # Errors
    /// Refuses controls without valid bounds.
    pub fn location(&mut self) -> Result<(f64, f64), SkipReason> {
        let element = self.focus()?;
        let rect = element
            .get_bounding_rectangle()
            .map_err(|_| SkipReason::UnsupportedWrite)?;
        if rect.get_width() <= 0 || rect.get_height() <= 0 {
            return Err(SkipReason::UnknownField);
        }
        let bottom = rect
            .get_top()
            .checked_add(rect.get_height())
            .ok_or(SkipReason::UnknownField)?;
        Ok((f64::from(rect.get_left()), f64::from(bottom)))
    }

    /// Runs a listen-only keyboard hook on the calling thread.
    /// # Errors
    /// Refuses a failed hook installation.
    pub fn observe(
        active: &Arc<AtomicBool>,
        ready: &Arc<AtomicBool>,
        gate: &Gate,
        handler: &Handler,
    ) -> Result<(), SkipReason> {
        TAP.with(|context| {
            *context.borrow_mut() = Some(Hook {
                active: active.clone(),
                gate: gate.clone(),
                handler: handler.clone(),
            });
        });
        let hook =
            // SAFETY: The callback has the required ABI and its thread-local context outlives the hook and message loop.
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(callback), std::ptr::null_mut(), 0) };
        if hook.is_null() {
            TAP.with(|context| *context.borrow_mut() = None);
            return Err(SkipReason::PermissionMissing);
        }
        ready.store(true, Ordering::Release);
        // SAFETY: MSG is a C record of integers and pointers, all of which
        // permit zero initialization.
        let mut message = unsafe { std::mem::zeroed::<MSG>() };
        // SAFETY: message is a writable MSG; the null window selects this
        // thread's message queue.
        while unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) } > 0 {
            // SAFETY: The initialized message came from GetMessageW on this
            // thread and stays live during dispatch.
            unsafe {
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
        ready.store(false, Ordering::Release);
        // SAFETY: The handle came from a successful installation and its
        // context is still alive.
        unsafe {
            UnhookWindowsHookEx(hook);
        }
        TAP.with(|context| *context.borrow_mut() = None);
        Ok(())
    }
}
fn target(element: &UIElement) -> Result<TargetId, SkipReason> {
    let runtime = element
        .get_runtime_id()
        .map_err(|_| SkipReason::UnknownField)?;
    let pid = element
        .get_process_id()
        .map_err(|_| SkipReason::UnknownField)?;
    Ok(super::identity_token(format!("{pid}:{runtime:?}")))
}
struct Process(windows_sys::Win32::Foundation::HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: Process owns this valid handle exclusively and closes it
        // exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn application(pid: u32) -> Result<Application, SkipReason> {
    // SAFETY: The PID is native metadata and only limited query access is
    // requested; the handle is checked and closed.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(SkipReason::UnknownField);
    }
    let process = Process(process);
    let mut path = vec![0u16; 32768];
    let mut length = 32768;
    // SAFETY: The path buffer contains length writable UTF-16 units and the
    // length output is valid.
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &raw mut length) } == 0
    {
        return Err(SkipReason::UnknownField);
    }
    let path =
        String::from_utf16(&path[..length as usize]).map_err(|_| SkipReason::UnknownField)?;
    let mut length = 0;
    let result =
        // SAFETY: The process handle is live and the supplied output length describes either a null size probe or the allocated UTF-16 buffer.
        unsafe { GetApplicationUserModelId(process.0, &raw mut length, std::ptr::null_mut()) };
    let identity = if result == 15703 {
        Identity::WindowsExecutable(path.clone())
    } else if result == 122 && length > 0 {
        let mut id = vec![0u16; length as usize];
        // SAFETY: The process handle is live and the supplied output length
        // describes either a null size probe or the allocated UTF-16 buffer.
        if unsafe { GetApplicationUserModelId(process.0, &raw mut length, id.as_mut_ptr()) } != 0 {
            return Err(SkipReason::UnknownField);
        }
        let end = id
            .iter()
            .position(|value| *value == 0)
            .ok_or(SkipReason::UnknownField)?;
        Identity::WindowsApp(String::from_utf16(&id[..end]).map_err(|_| SkipReason::UnknownField)?)
    } else {
        return Err(SkipReason::UnknownField);
    };
    let name = std::path::Path::new(&path)
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or(SkipReason::UnknownField)?
        .to_owned();
    Ok(Application { identity, name })
}
#[must_use]
pub fn available_applications() -> Vec<Application> {
    let mut pids = vec![0u32; 16384];
    let mut bytes = 0;
    // SAFETY: The buffer has 16384 u32 slots, exactly the 65536 bytes supplied;
    // bytes is a valid output.
    if unsafe { K32EnumProcesses(pids.as_mut_ptr(), 65536, &raw mut bytes) } == 0 {
        return Vec::new();
    }
    let own = std::process::id();
    pids[..(bytes as usize / 4).min(pids.len())]
        .iter()
        .filter(|pid| **pid != own)
        .filter_map(|pid| application(*pid).ok())
        .collect()
}
impl Accessibility for Adapter {
    fn select_input_language(&mut self, expected: &FieldSnapshot, language: Language) -> bool {
        if self.snapshot().as_ref() != Ok(expected) {
            return false;
        }
        self.select_language(expected, language).is_some()
    }

    fn snapshot(&mut self) -> Result<FieldSnapshot, SkipReason> {
        let element = self.focus()?;
        let application = application(
            element
                .get_process_id()
                .map_err(|_| SkipReason::UnknownField)?,
        )?;
        if self.allowed.as_ref() != Some(&application.identity) {
            return Err(SkipReason::ExcludedApp);
        }
        if let Ok(edit) = element.get_pattern::<uiautomation::patterns::UITextEditPattern>()
            && let Ok(composition) = edit.get_active_composition()
            && !composition
                .get_text(-1)
                .map_err(|_| SkipReason::UnknownField)?
                .is_empty()
        {
            return Err(SkipReason::Composing);
        }
        let value = element
            .get_pattern::<UIValuePattern>()
            .map_err(|_| SkipReason::UnsupportedWrite)?
            .get_value()
            .map_err(|_| SkipReason::UnknownField)?;
        let text = element
            .get_pattern::<UITextPattern>()
            .map_err(|_| SkipReason::UnsupportedWrite)?;
        let selections = text.get_selection().map_err(|_| SkipReason::UnknownField)?;
        if selections.len() != 1 {
            return Err(SkipReason::UnknownField);
        }
        let full = text
            .get_document_range()
            .map_err(|_| SkipReason::UnknownField)?;
        if full.get_text(-1).map_err(|_| SkipReason::UnknownField)? != value {
            return Err(SkipReason::UnknownField);
        }
        let prefix = text
            .get_document_range()
            .map_err(|_| SkipReason::UnknownField)?;
        prefix
            .move_endpoint_by_range(
                TextPatternRangeEndpoint::End,
                &selections[0],
                TextPatternRangeEndpoint::Start,
            )
            .map_err(|_| SkipReason::UnknownField)?;
        let caret = prefix
            .get_text(-1)
            .map_err(|_| SkipReason::UnknownField)?
            .chars()
            .count();
        let selection_len = selections[0]
            .get_text(-1)
            .map_err(|_| SkipReason::UnknownField)?
            .chars()
            .count();
        let snapshot = FieldSnapshot {
            target: target(&element)?,
            sensitivity: FieldSensitivity::Normal,
            supports_write: true,
            text: value,
            caret,
            selection_len,
        };
        self.last = Some((
            element
                .get_runtime_id()
                .map_err(|_| SkipReason::UnknownField)?,
            snapshot.clone(),
        ));
        Ok(snapshot)
    }

    fn replace(
        &mut self,
        id: TargetId,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> Result<(), WriteError> {
        let Some((runtime_id, before)) = self.last.take() else {
            return Err(WriteError::Failed);
        };
        let element = self.focus().map_err(|_| WriteError::Failed)?;
        if element.get_runtime_id().map_err(|_| WriteError::Failed)? != runtime_id
            || before.target != id
            || end < start
            || end > before.text.chars().count()
            || self.snapshot().map_err(|_| WriteError::Failed)? != before
        {
            return Err(WriteError::Failed);
        }
        let mut chars: Vec<char> = before.text.chars().collect();
        chars.splice(start..end, replacement.chars());
        let expected: String = chars.into_iter().collect();
        let value = element
            .get_pattern::<UIValuePattern>()
            .map_err(|_| WriteError::Failed)?;
        if value.set_value(&expected).is_err() {
            return Err(WriteError::Uncertain);
        }
        if value.get_value().map_err(|_| WriteError::Uncertain)? != expected {
            return Err(WriteError::Uncertain);
        }
        let text = element
            .get_pattern::<UITextPattern>()
            .map_err(|_| WriteError::Uncertain)?;
        let caret = before.caret.saturating_sub(end - start) + replacement.chars().count();
        let offset = i32::try_from(caret).map_err(|_| WriteError::Uncertain)?;
        let range = text
            .get_document_range()
            .map_err(|_| WriteError::Uncertain)?;
        range
            .move_endpoint_by_range(
                TextPatternRangeEndpoint::End,
                &range,
                TextPatternRangeEndpoint::Start,
            )
            .map_err(|_| WriteError::Uncertain)?;
        let moved = range
            .move_endpoint_by_unit(
                TextPatternRangeEndpoint::Start,
                uiautomation::types::TextUnit::Character,
                offset,
            )
            .map_err(|_| WriteError::Uncertain)?;
        if moved != offset {
            return Err(WriteError::Partial {
                written: replacement.into(),
            });
        }
        if self
            .focus()
            .map_err(|_| WriteError::Uncertain)?
            .get_runtime_id()
            .map_err(|_| WriteError::Uncertain)?
            != runtime_id
        {
            return Err(WriteError::Uncertain);
        }
        range.select().map_err(|_| WriteError::Partial {
            written: replacement.into(),
        })?;
        let after = self.snapshot().map_err(|_| WriteError::Uncertain)?;
        if after.target != id
            || after.text != expected
            || after.caret != caret
            || after.selection_len != 0
        {
            return Err(WriteError::Uncertain);
        }
        Ok(())
    }
}
struct Hook {
    active: Arc<AtomicBool>,
    gate: Gate,
    handler: Handler,
}
thread_local! { static TAP:RefCell<Option<Hook>>=const { RefCell::new(None) }; }
unsafe extern "system" fn callback(code: i32, wparam: usize, lparam: isize) -> isize {
    let _ = std::panic::catch_unwind(|| {
        TAP.with(|context| {
            let context = context.borrow();
            let Some(context) = &*context else { return };
            if code != 0
                || (wparam != WM_KEYDOWN as usize && wparam != WM_SYSKEYDOWN as usize)
                || !context.active.load(Ordering::Acquire)
            {
                return;
            }
            let observed_at = std::time::Instant::now();
            let mut adapter = Adapter::new();
            let Ok((target, application)) = adapter.metadata() else {
                return;
            };
            // SAFETY: Only documented modifier virtual-key constants are
            // queried after the safety gate.
            if !(context.gate)(&application) {
                return;
            }
            // SAFETY: HC_ACTION plus a key-down message establishes the
            // Windows-provided KBDLLHOOKSTRUCT pointer, borrowed only for this
            // callback.
            let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            // SAFETY: Query the documented Control modifier after the safety
            // gate.
            let ctrl = unsafe { GetAsyncKeyState(0x11) } < 0;
            // SAFETY: Only documented modifier virtual-key constants are
            // queried after the safety gate.
            let alt = unsafe { GetAsyncKeyState(0x12) } < 0;
            let meta =
                // SAFETY: Only documented modifier virtual-key constants are queried after the safety gate.
                unsafe { GetAsyncKeyState(0x5b) } < 0 || unsafe { GetAsyncKeyState(0x5c) } < 0;
            // SAFETY: Only documented modifier virtual-key constants are
            // queried after the safety gate.
            let shift = unsafe { GetAsyncKeyState(0x10) } < 0;
            // SAFETY: The documented Caps Lock virtual key is queried after the
            // safety gate.
            let caps = unsafe { GetKeyState(0x14) } & 1 != 0;
            let key = if event.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
                Key::Untrusted
            } else if (ctrl && event.vkCode == 0x56) || (shift && event.vkCode == 0x2d) {
                Key::Paste
            } else if ctrl || alt || meta {
                Key::Reset
            } else {
                super::pc_key(event.scanCode, shift, caps)
            };
            if !matches!(key, Key::Modifier) {
                (context.handler)(KeyEvent {
                    key,
                    target,
                    observed_at,
                });
            }
        });
    });
    // SAFETY: Forward the unchanged callback parameters; FixKey never consumes
    // or modifies input.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{
        Adapter, GET_CONVERSION_MODE, GET_OPEN_STATUS, HKL, IMC_SETCONVERSIONMODE,
        IME_CMODE_NATIVE, IUIAutomation, IUIAutomation2, Interface, Language, layout_matches,
        preferred_layout, select_ime_mode,
    };

    #[test]
    fn input_layouts_accept_only_us_qwerty_and_microsoft_korean() {
        for identifier in [0x0409_0409usize, 0x0412_0412, 0xe001_0412] {
            let language = if identifier == 0x0409_0409 {
                Language::EnUsQwerty
            } else {
                Language::KoDubeolsik
            };
            assert!(layout_matches(identifier as HKL, language));
        }
        for identifier in [
            0usize,
            0x0409,
            0x0412,
            0xf002_0409,
            0x0809_0809,
            0xe020_0412,
            0xe001_0411,
        ] {
            for language in [Language::EnUsQwerty, Language::KoDubeolsik] {
                assert!(!layout_matches(identifier as HKL, language));
            }
        }
    }

    #[test]
    fn input_layout_selection_preserves_loaded_sources_and_current_preference() {
        let english = 0x0409_0409usize as HKL;
        let korean = 0x0412_0412usize as HKL;
        let legacy_korean = 0xe001_0412usize as HKL;
        let layouts = [english, korean, legacy_korean];
        assert_eq!(
            preferred_layout(&layouts, legacy_korean, Language::KoDubeolsik),
            Some(legacy_korean)
        );
        assert_eq!(
            preferred_layout(&layouts, english, Language::KoDubeolsik),
            Some(korean)
        );
        assert_eq!(
            preferred_layout(&layouts, korean, Language::EnUsQwerty),
            Some(english)
        );
        assert_eq!(
            preferred_layout(&[english], english, Language::KoDubeolsik),
            None
        );
        assert_eq!(preferred_layout(&[], english, Language::EnUsQwerty), None);
    }

    #[test]
    fn ime_modes_switch_both_directions_and_preserve_other_flags() {
        for (language, initial, expected) in [
            (Language::KoDubeolsik, 0x10usize, 0x11usize),
            (Language::EnUsQwerty, 0x11usize, 0x10usize),
        ] {
            let mut mode = initial;
            let mut writes = 0;
            let result = select_ime_mode(language, |command, value| match command {
                GET_OPEN_STATUS => Some(1),
                GET_CONVERSION_MODE => Some(mode),
                command if command == IMC_SETCONVERSIONMODE as usize => {
                    mode = usize::try_from(value).unwrap();
                    writes += 1;
                    Some(0)
                }
                _ => panic!("unexpected IME command"),
            });
            assert_eq!(result, Some(()));
            assert_eq!(mode, expected);
            assert_eq!(writes, 1);
        }
    }

    #[test]
    fn ime_modes_refuse_closed_unsupported_or_timed_out_queries_without_writes() {
        for response in [None, Some(0), Some(2)] {
            let mut calls = 0;
            assert_eq!(
                select_ime_mode(Language::KoDubeolsik, |command, _| {
                    calls += 1;
                    assert_eq!(command, GET_OPEN_STATUS);
                    response
                }),
                None
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn ime_modes_verify_changes_and_never_retry_failures() {
        for response in [None, Some(0), Some(1)] {
            let mut writes = 0;
            let result = select_ime_mode(Language::KoDubeolsik, |command, _| match command {
                GET_OPEN_STATUS => Some(1),
                GET_CONVERSION_MODE => Some(0),
                command if command == IMC_SETCONVERSIONMODE as usize => {
                    writes += 1;
                    response
                }
                _ => panic!("unexpected IME command"),
            });
            assert_eq!(result, None);
            assert_eq!(writes, 1);
        }
    }

    #[test]
    fn ime_modes_do_not_write_when_already_selected() {
        for language in [Language::KoDubeolsik, Language::EnUsQwerty] {
            let mode = if language == Language::KoDubeolsik {
                IME_CMODE_NATIVE as usize
            } else {
                0
            };
            assert_eq!(
                select_ime_mode(language, |command, _| {
                    match command {
                        GET_OPEN_STATUS => Some(1),
                        GET_CONVERSION_MODE => Some(mode),
                        _ => panic!("must not change an already selected mode"),
                    }
                }),
                Some(())
            );
        }
    }

    #[test]
    fn english_selection_can_use_korean_ime_without_a_loaded_us_layout() {
        let korean = 0x0412_0412usize as HKL;
        assert_eq!(
            preferred_layout(&[korean], korean, Language::EnUsQwerty),
            Some(korean)
        );
    }

    #[test]
    fn initializes_bounded_automation_on_windows() {
        let adapter = Adapter::new();
        assert!(
            adapter.initialized,
            "Windows COM must initialize on the adapter thread"
        );
        let automation = adapter
            .automation
            .as_ref()
            .expect("Windows UI Automation must support finite provider timeouts");
        let raw: &IUIAutomation = automation.as_ref();
        let bounded = raw.cast::<IUIAutomation2>().unwrap();
        // SAFETY: The adapter owns this live COM interface on the current thread.
        assert_eq!(unsafe { bounded.ConnectionTimeout() }.unwrap(), 50);
        // SAFETY: The adapter owns this live COM interface on the current thread.
        assert_eq!(unsafe { bounded.TransactionTimeout() }.unwrap(), 50);
    }
}
