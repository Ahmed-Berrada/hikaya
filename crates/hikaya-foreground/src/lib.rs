//! Which app has the keyboard right now.
//!
//! Hikaya asks when a dictation starts, so the text can be cleaned for the
//! place it goes and replacement rules scoped to an app can apply. Only
//! Windows answers; elsewhere the answer is "unknown" and dictation works
//! as before. On Linux, Wayland gives no app a way to ask.

use hikaya_core::context::Foreground;

/// The focused window's app and title, if the system says.
pub fn foreground() -> Option<Foreground> {
    imp::foreground()
}

#[cfg(windows)]
mod imp {
    use super::Foreground;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    pub fn foreground() -> Option<Foreground> {
        // SAFETY: GetForegroundWindow takes no arguments and may return a
        // null handle, which is checked before use.
        let window = unsafe { GetForegroundWindow() };
        if window.is_invalid() {
            return None;
        }
        let mut process_id = 0u32;
        // SAFETY: `window` is a window handle from the call above and the
        // out pointer refers to a live local.
        if unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) } == 0 {
            return None;
        }
        let process = process_name(process_id)?;
        let mut title = [0u16; 512];
        // SAFETY: the buffer is a live local and its length bounds the write.
        let length = unsafe { GetWindowTextW(window, &mut title) };
        let title = String::from_utf16_lossy(&title[..usize::try_from(length).unwrap_or(0)]);
        Some(Foreground { process, title })
    }

    /// The executable's file stem, lowercase: `C:\...\Slack.exe` → `slack`.
    fn process_name(process_id: u32) -> Option<String> {
        // SAFETY: OpenProcess only reads its arguments; a failure is an Err.
        let process: HANDLE =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
        let mut path = [0u16; 1024];
        let mut length = path.len() as u32;
        // SAFETY: `process` was opened above with the query right this call
        // needs; the buffer and its length describe a live local.
        let read = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        };
        // SAFETY: `process` is a handle this function opened and owns.
        let _ = unsafe { CloseHandle(process) };
        read.ok()?;
        let path = String::from_utf16_lossy(&path[..length as usize]);
        let file = path.rsplit(['\\', '/']).next()?;
        let stem = file
            .len()
            .checked_sub(4)
            .filter(|&end| {
                file.get(end..)
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(".exe"))
            })
            .map_or(file, |end| &file[..end]);
        Some(stem.to_lowercase())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Foreground;

    pub fn foreground() -> Option<Foreground> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn answers_without_panicking() {
        // On a CI machine there may be no window at all; any answer is fine.
        let _ = super::foreground();
    }
}
