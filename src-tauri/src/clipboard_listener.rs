use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::database::{content_hash, Database};

/// 自己写进剪贴板的内容，在这个时间窗内被监听到就当成回写忽略。
/// 超时未消费的条目自动失效，避免暂停监听后 hash 永久残留。
const IGNORED_WRITE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ClipboardListener {
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashMap<String, Instant>>>,
}

impl ClipboardListener {
    pub fn start(app: AppHandle) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            let paused = Arc::new(AtomicBool::new(false));
            let ignored_hashes = Arc::new(Mutex::new(HashMap::new()));
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
        let previous = self.paused.swap(paused, Ordering::Relaxed);
        // 暂停期间不会消费 ignore 标记，切换状态时直接清空，防止恢复监听后吞掉一次真实复制
        if previous != paused {
            if let Ok(mut hashes) = self.ignored_hashes.lock() {
                hashes.clear();
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn ignore_next_write(&self, content: &str) {
        if let Ok(mut hashes) = self.ignored_hashes.lock() {
            let now = Instant::now();
            hashes.retain(|_, inserted| now.duration_since(*inserted) < IGNORED_WRITE_TTL);
            hashes.insert(content_hash(content), now);
        }
    }

    pub fn cancel_ignored_write(&self, content: &str) {
        if let Ok(mut hashes) = self.ignored_hashes.lock() {
            hashes.remove(&content_hash(content));
        }
    }
}

/// 消费一次 ignore 标记。返回 true 表示这次剪贴板变化是本程序自己写进去的。
fn consume_ignored_write(ignored_hashes: &Mutex<HashMap<String, Instant>>, content: &str) -> bool {
    let hash = content_hash(content);
    if let Ok(mut hashes) = ignored_hashes.lock() {
        if let Some(inserted) = hashes.remove(&hash) {
            return inserted.elapsed() < IGNORED_WRITE_TTL;
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn start_windows_listener(
    app: AppHandle,
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashMap<String, Instant>>>,
) -> Result<(), String> {
    use std::sync::mpsc::sync_channel;
    use std::thread;

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
    ignored_hashes: Arc<Mutex<HashMap<String, Instant>>>,
}

#[cfg(target_os = "windows")]
unsafe fn run_message_loop(
    app: AppHandle,
    paused: Arc<AtomicBool>,
    ignored_hashes: Arc<Mutex<HashMap<String, Instant>>>,
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

        if self.paused.load(Ordering::Relaxed) {
            return;
        }

        // 部分应用是延迟渲染剪贴板内容的，需要读不到时再重试。
        // 这里先立刻读一次，失败后按 5/10/20ms 退避，总占用 35ms。
        // 原实现固定先等 40ms，连续快速复制时会丢掉中间的事件。
        let mut last_error = String::new();
        for delay in [0u64, 5, 10, 20] {
            if delay > 0 {
                thread::sleep(Duration::from_millis(delay));
            }
            match read_unicode_clipboard() {
                Ok(Some(content)) => {
                    if consume_ignored_write(&self.ignored_hashes, &content) {
                        return;
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
                Err(error) => last_error = error,
            }
        }
        eprintln!("读取 Windows 剪贴板失败: {last_error}");
    }
}

#[cfg(target_os = "windows")]
fn read_unicode_clipboard() -> Result<Option<String>, String> {
    use std::slice;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock, GlobalSize};

    unsafe {
        const CF_UNICODETEXT: u32 = 13;
        OpenClipboard(None).map_err(|error| error.to_string())?;
        let result = (|| {
            // 密码管理器会打上"排除剪贴板历史"标记，Windows 自带历史会跳过，这里也必须跳过
            if clipboard_excluded_from_history() {
                return Ok(None);
            }

            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                return Ok(None);
            }

            let handle = GetClipboardData(CF_UNICODETEXT).map_err(|error| error.to_string())?;
            let pointer = GlobalLock(HGLOBAL(handle.0));
            if pointer.is_null() {
                return Err("无法锁定 Windows 剪贴板内存".to_string());
            }

            // 用 GlobalSize 定上界：剪贴板内存由别的进程提供，
            // 若没有 null 结尾，靠"扫到 0 为止"会越界读。
            let size = GlobalSize(HGLOBAL(handle.0));
            let max_units = size / std::mem::size_of::<u16>();
            let mut length = 0usize;
            while length < max_units && *(pointer as *const u16).add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(slice::from_raw_parts(pointer as *const u16, length));

            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Ok(Some(text))
        })();
        let _ = CloseClipboard();
        result
    }
}

/// 检查 Windows 剪贴板的"不要记录"标记。
/// 调用前必须已经 OpenClipboard。
#[cfg(target_os = "windows")]
fn clipboard_excluded_from_history() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        GetClipboardData, IsClipboardFormatAvailable, RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    unsafe {
        // 标记一：ExcludeClipboardContentFromMonitorProcessing（存在即排除）
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        if exclude != 0 && IsClipboardFormatAvailable(exclude).is_ok() {
            return true;
        }

        // 标记二：CanIncludeInClipboardHistory == 0（显式声明不要进历史）
        let can_include = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if can_include != 0 && IsClipboardFormatAvailable(can_include).is_ok() {
            if let Ok(handle) = GetClipboardData(can_include) {
                let pointer = GlobalLock(HGLOBAL(handle.0));
                if !pointer.is_null() {
                    let flag = *(pointer as *const u32);
                    let _ = GlobalUnlock(HGLOBAL(handle.0));
                    return flag == 0;
                }
            }
        }

        false
    }
}
