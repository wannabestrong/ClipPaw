use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::database::{content_hash, Database};

#[derive(Clone)]
pub struct ClipboardListener {
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashSet<String>>>,
}

impl ClipboardListener {
    pub fn start(app: AppHandle) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            let paused = Arc::new(AtomicBool::new(false));
            let ignored_hashes = Arc::new(Mutex::new(HashSet::new()));
            start_windows_listener(app, Arc::clone(&paused), Arc::clone(&ignored_hashes))?;
            return Ok(Self {
                paused,
                ignored_hashes,
            });
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = app;
            Err("剪贴板监听当前仅支持 Windows".to_string())
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn ignore_next_write(&self, content: &str) {
        if let Ok(mut hashes) = self.ignored_hashes.lock() {
            hashes.insert(content_hash(content));
        }
    }

    pub fn cancel_ignored_write(&self, content: &str) {
        if let Ok(mut hashes) = self.ignored_hashes.lock() {
            hashes.remove(&content_hash(content));
        }
    }
}

#[cfg(target_os = "windows")]
fn start_windows_listener(
    app: AppHandle,
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashSet<String>>>,
) -> Result<(), String> {
    use std::sync::mpsc::sync_channel;
    use std::thread;
    use std::time::Duration;

    let (ready_tx, ready_rx) = sync_channel(1);
    thread::Builder::new()
        .name("clipboard-listener".to_string())
        .spawn(move || {
            let result = unsafe { run_message_loop(app, paused, ignored_hashes, ready_tx.clone()) };
            if let Err(error) = result {
                let _ = ready_tx.send(Err(error));
            }
        })
        .map_err(|error| format!("无法启动剪贴板监听线程: {error}"))?;

    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "剪贴板监听线程启动超时".to_string())?
        .map_err(|error| format!("初始化 Windows 剪贴板监听失败: {error}"))
}

#[cfg(target_os = "windows")]
struct ListenerContext {
    app: AppHandle,
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashSet<String>>>,
}

#[cfg(target_os = "windows")]
unsafe fn run_message_loop(
    app: AppHandle,
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashSet<String>>>,
    ready_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::DataExchange::AddClipboardFormatListener;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage,
        CS_HREDRAW, CS_VREDRAW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    let class_name = w!("ClipboardHistoryListener");
    let instance = HINSTANCE(GetModuleHandleW(None).map_err(|error| error.to_string())?.0);
    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    RegisterClassW(&window_class);

    let context = Box::new(ListenerContext {
        app,
        paused,
        ignored_hashes,
    });
    let context_ptr = Box::into_raw(context);
    let window = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        class_name,
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(instance),
        Some(context_ptr.cast::<c_void>()),
    ) {
        Ok(window) => window,
        Err(error) => {
            drop(Box::from_raw(context_ptr));
            return Err(error.to_string());
        }
    };

    if let Err(error) = AddClipboardFormatListener(window) {
        drop(Box::from_raw(context_ptr));
        return Err(error.to_string());
    }

    ready_tx
        .send(Ok(()))
        .map_err(|_| "无法报告剪贴板监听启动状态".to_string())?;

    let mut message = MSG::default();
    loop {
        let result = GetMessageW(&mut message, None, 0, 0).0;
        if result <= 0 {
            break;
        }
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    Ok(())
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn window_proc(
    window: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use std::ffi::c_void;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, CREATESTRUCTW, GWLP_USERDATA,
        WM_NCCREATE,
    };

    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        return LRESULT(1);
    }

    if message == windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE {
        let context_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *const ListenerContext;
        if !context_ptr.is_null() {
            let context = &*context_ptr;
            context.handle_update();
        }
        return LRESULT(0);
    }

    if message == windows::Win32::UI::WindowsAndMessaging::WM_NCDESTROY {
        let context_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut c_void;
        if !context_ptr.is_null() {
            let _ = Box::from_raw(context_ptr as *mut ListenerContext);
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        }
    }

    DefWindowProcW(window, message, wparam, lparam)
}

#[cfg(target_os = "windows")]
impl ListenerContext {
    fn handle_update(&self) {
        use std::thread;
        use std::time::Duration;

        if self.paused.load(Ordering::Relaxed) {
            return;
        }

        // Some applications publish clipboard data asynchronously. Retry briefly
        // instead of polling continuously, while keeping the UI thread untouched.
        thread::sleep(Duration::from_millis(40));
        for attempt in 0..4 {
            match read_unicode_clipboard() {
                Ok(Some(content)) => {
                    let hash = content_hash(&content);
                    if let Ok(mut ignored_hashes) = self.ignored_hashes.lock() {
                        if ignored_hashes.remove(&hash) {
                            return;
                        }
                    }
                    let database = self.app.state::<Database>();
                    match database.upsert_text(&content) {
                        Ok(record) => {
                            let _ = self.app.emit("clipboard-updated", &record);
                        }
                        Err(error) => {
                            eprintln!("保存剪贴板记录失败: {error}");
                        }
                    }
                    return;
                }
                Ok(None) => return,
                Err(_) if attempt < 3 => thread::sleep(Duration::from_millis(15)),
                Err(error) => {
                    eprintln!("读取 Windows 剪贴板失败: {error}");
                    return;
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn read_unicode_clipboard() -> Result<Option<String>, String> {
    use std::slice;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    unsafe {
        const CF_UNICODETEXT: u32 = 13;
        OpenClipboard(None).map_err(|error| error.to_string())?;
        let result = (|| {
            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                return Ok(None);
            }

            let handle = GetClipboardData(CF_UNICODETEXT).map_err(|error| error.to_string())?;
            let pointer = GlobalLock(HGLOBAL(handle.0));
            if pointer.is_null() {
                return Err("无法锁定 Windows 剪贴板内存".to_string());
            }

            let mut length = 0;
            let text = loop {
                let character = *(pointer as *const u16).add(length);
                if character == 0 {
                    break String::from_utf16_lossy(slice::from_raw_parts(
                        pointer as *const u16,
                        length,
                    ));
                }
                length += 1;
            };

            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Ok(Some(text))
        })();
        let _ = CloseClipboard();
        result
    }
}
