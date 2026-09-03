#[cfg(target_os = "windows")]
pub fn write_unicode_text(content: &str) -> Result<(), String> {
    use std::thread;
    use std::time::Duration;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    let mut last_error = String::new();
    for attempt in 0..5 {
        unsafe {
            match OpenClipboard(None) {
                Ok(()) => {
                    let result = (|| {
                        EmptyClipboard().map_err(|error| error.to_string())?;
                        let utf16: Vec<u16> = content.encode_utf16().chain([0]).collect();
                        let memory = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * size_of::<u16>())
                            .map_err(|error| error.to_string())?;
                        let pointer = GlobalLock(memory);
                        if pointer.is_null() {
                            return Err("无法分配 Windows 剪贴板内存".to_string());
                        }
                        std::ptr::copy_nonoverlapping(
                            utf16.as_ptr(),
                            pointer.cast::<u16>(),
                            utf16.len(),
                        );
                        let _ = GlobalUnlock(memory);
                        SetClipboardData(13, Some(HANDLE(memory.0)))
                            .map_err(|error| error.to_string())?;
                        Ok(())
                    })();
                    let _ = CloseClipboard();
                    return result;
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        if attempt < 4 {
            thread::sleep(Duration::from_millis(15));
        }
    }
    Err(format!("无法打开 Windows 剪贴板: {last_error}"))
}

#[cfg(not(target_os = "windows"))]
pub fn write_unicode_text(_content: &str) -> Result<(), String> {
    Err("写入系统剪贴板当前仅支持 Windows".to_string())
}
