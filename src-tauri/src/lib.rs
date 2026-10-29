mod clipboard_listener;
mod clipboard_writer;
mod database;

use clipboard_listener::ClipboardListener;
use database::{AppSettings, ClipboardRecord, Database, ListQuery};
use std::str::FromStr;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

/// 开机自启动时附加的标记，用于区分"用户手动启动"和"登录时自启"。
/// 自启时不弹窗，只在托盘里待着。
const AUTOSTART_FLAG: &str = "--autostart";

#[tauri::command]
fn list_clipboard_items(
    database: State<'_, Database>,
    query: Option<ListQuery>,
) -> Result<Vec<ClipboardRecord>, String> {
    database.list(query.unwrap_or(ListQuery {
        search: None,
        limit: None,
    }))
}

#[tauri::command]
fn delete_clipboard_item(database: State<'_, Database>, id: i64) -> Result<bool, String> {
    database.delete(id)
}

#[tauri::command]
fn set_clipboard_item_pinned(
    database: State<'_, Database>,
    id: i64,
    pinned: bool,
) -> Result<bool, String> {
    database.set_pinned(id, pinned)
}

#[tauri::command]
fn clear_clipboard_history(database: State<'_, Database>) -> Result<u64, String> {
    database.clear()
}

#[tauri::command]
fn get_app_settings(database: State<'_, Database>) -> Result<AppSettings, String> {
    database.settings()
}

#[tauri::command]
fn update_app_settings(
    app: AppHandle,
    database: State<'_, Database>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let updated = database.update_settings(settings)?;
    database.cleanup(&updated)?;
    refresh_tray_menu(&app, &updated.language);
    Ok(updated)
}

#[tauri::command]
fn set_app_language(
    app: AppHandle,
    database: State<'_, Database>,
    language: String,
) -> Result<(), String> {
    let mut settings = database.settings()?;
    if settings.language == language {
        return Ok(());
    }
    settings.language = language;
    let updated = database.update_settings(settings)?;
    refresh_tray_menu(&app, &updated.language);
    Ok(())
}

#[tauri::command]
fn set_clipboard_listener_paused(
    app: AppHandle,
    database: State<'_, Database>,
    listener: State<'_, ClipboardListener>,
    paused: bool,
) -> bool {
    listener.set_paused(paused);
    let current = listener.is_paused();
    let _ = app.emit("listener-paused-changed", current);
    if let Ok(settings) = database.settings() {
        refresh_tray_menu(&app, &settings.language);
    }
    current
}

#[tauri::command]
fn is_clipboard_listener_paused(listener: State<'_, ClipboardListener>) -> bool {
    listener.is_paused()
}

#[tauri::command]
fn write_clipboard_text(
    app: AppHandle,
    listener: State<'_, ClipboardListener>,
    database: State<'_, Database>,
    content: String,
) -> Result<(), String> {
    listener.ignore_next_write(&content);
    if let Err(error) = clipboard_writer::write_unicode_text(&content) {
        listener.cancel_ignored_write(&content);
        return Err(error);
    }

    // 从历史复制，等于这条内容又进了一次剪贴板，和外部复制保持一致：
    // 刷新 updated_at 让它回到顶部，同时重置按天清理的计时（否则常用条目会被当成"很久没用"删掉）。
    // 监听线程那边已经被 ignore 标记拦住，这里手动补一次，避免重复写入。
    match database.upsert_text(&content) {
        Ok(record) => {
            let _ = app.emit("clipboard-updated", &record);
        }
        Err(error) => eprintln!("刷新剪贴板记录时间失败: {error}"),
    }
    Ok(())
}

#[tauri::command]
fn get_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let current = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;
    if enabled && !current {
        app.autolaunch()
            .enable()
            .map_err(|error| error.to_string())?;
    } else if !enabled && current {
        app.autolaunch()
            .disable()
            .map_err(|error| error.to_string())?;
    }
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

struct TrayLabels {
    open: &'static str,
    settings: &'static str,
    pause: &'static str,
    resume: &'static str,
    quit: &'static str,
}

fn tray_labels(language: &str) -> TrayLabels {
    match language {
        "en" => TrayLabels {
            open: "Open Clipboard",
            settings: "Settings",
            pause: "Pause Listening",
            resume: "Resume Listening",
            quit: "Quit",
        },
        _ => TrayLabels {
            open: "打开剪贴板",
            settings: "设置",
            pause: "暂停监听",
            resume: "恢复监听",
            quit: "退出",
        },
    }
}

/// 托盘图标句柄。Tauri 的 tray_by_id 受 feature/版本影响，
/// 自己持有 TrayIcon（Clone + Send + Sync）更稳妥。
struct TrayState(Mutex<Option<tauri::tray::TrayIcon<tauri::Wry>>>);

fn build_tray_menu(
    app: &AppHandle,
    language: &str,
    paused: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let labels = tray_labels(language);
    tauri::menu::MenuBuilder::new(app)
        .text("open", labels.open)
        .text("settings", labels.settings)
        .separator()
        .text("pause", if paused { labels.resume } else { labels.pause })
        .separator()
        .text("quit", labels.quit)
        .build()
}

/// 托盘菜单文案依赖语言和暂停状态，这两者变化时都要重建菜单。
fn refresh_tray_menu(app: &AppHandle, language: &str) {
    let paused = app
        .try_state::<ClipboardListener>()
        .map(|listener| listener.is_paused())
        .unwrap_or(false);
    let Some(tray_state) = app.try_state::<TrayState>() else {
        return;
    };
    let Ok(menu) = build_tray_menu(app, language, paused) else {
        return;
    };
    // 这里必须用 let-else 绑定成具名变量：写成 if let Ok(guard) = ... 时，
    // 条件里的 Result 临时值会活到函数末尾，比 tray_state 更晚析构，触发 E0597。
    let Ok(guard) = tray_state.0.lock() else {
        return;
    };
    if let Some(tray) = guard.as_ref() {
        let _ = tray.set_menu(Some(menu));
    }
}

/// 读取 HKCU / HKLM 的 Run 项里 ClipPaw 的命令行。
#[cfg(target_os = "windows")]
fn autostart_entry_command() -> Option<String> {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let subkey = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run");
    let value_name = w!("ClipPaw");

    unsafe {
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut size: u32 = 0;
            if RegGetValueW(root, subkey, value_name, RRF_RT_REG_SZ, None, None, Some(&mut size)).0
                != 0
                || size == 0
            {
                continue;
            }
            let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
            if RegGetValueW(
                root,
                subkey,
                value_name,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .0
                != 0
            {
                continue;
            }
            let length = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            return Some(String::from_utf16_lossy(&buffer[..length]));
        }
    }
    None
}

/// 旧版本写入的自启动项没有 --autostart 参数，登录时仍会弹窗。
/// 只有"注册表里那条自启项指向的就是当前 exe"时才需要重写，
/// 否则从 target/release 或便携副本启动会把自启项指到临时路径。
#[cfg(target_os = "windows")]
fn autostart_needs_repair() -> bool {
    let Some(command) = autostart_entry_command() else {
        return false;
    };
    if command.contains(AUTOSTART_FLAG) {
        return false;
    }
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let current = current.to_string_lossy().to_lowercase();
    command.to_lowercase().contains(&current)
}

#[cfg(not(target_os = "windows"))]
fn autostart_needs_repair() -> bool {
    false
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkey = tauri_plugin_global_shortcut::Shortcut::from_str("CTRL+SHIFT+V")
        .expect("default global shortcut should be valid");
    let hotkey_id = hotkey.id();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("ClipPaw")
                .args([AUTOSTART_FLAG])
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut.id() == hotkey_id
                        && event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed
                    {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let database = Database::open(&app_data_dir).map_err(std::io::Error::other)?;
            let settings = database.settings().map_err(std::io::Error::other)?;
            let _ = database.cleanup(&settings).map_err(std::io::Error::other)?;
            app.manage(database);
            let listener =
                ClipboardListener::start(app.handle().clone()).map_err(std::io::Error::other)?;
            let listener_paused = listener.is_paused();
            app.manage(listener);
            if let Err(error) = app.global_shortcut().register(hotkey) {
                eprintln!("global shortcut registration failed: {error}");
            }

            let menu = build_tray_menu(app.handle(), &settings.language, listener_paused)?;
            let mut tray_builder = tauri::tray::TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("ClipPaw")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "settings" => {
                        let _ = app.emit("open-settings", ());
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "pause" => {
                        let listener = app.state::<ClipboardListener>();
                        let next = !listener.is_paused();
                        listener.set_paused(next);
                        // 前端和托盘菜单都要跟着变，否则界面显示与实际状态不符
                        let _ = app.emit("listener-paused-changed", next);
                        if let Ok(settings) = app.state::<Database>().settings() {
                            refresh_tray_menu(app, &settings.language);
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            let tray = tray_builder.build(app)?;
            app.manage(TrayState(Mutex::new(Some(tray))));

            // 旧版本写入的自启动项没有 --autostart 参数，登录时仍会弹窗。
            // 只在注册表那条自启项确实指向当前 exe 时才重写，避免把自启项指到临时路径。
            if !cfg!(debug_assertions) && autostart_needs_repair() {
                let _ = app.autolaunch().enable();
            }

            // 登录自启时保持隐藏、只驻留托盘；手动启动才显示窗口
            let launched_at_login = std::env::args().any(|arg| arg == AUTOSTART_FLAG);
            if !launched_at_login {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_clipboard_items,
            delete_clipboard_item,
            set_clipboard_item_pinned,
            clear_clipboard_history,
            set_clipboard_listener_paused,
            is_clipboard_listener_paused,
            write_clipboard_text,
            get_app_settings,
            update_app_settings,
            set_app_language,
            get_autostart_enabled,
            set_autostart_enabled
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
