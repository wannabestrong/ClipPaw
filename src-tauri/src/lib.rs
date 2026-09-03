mod clipboard_listener;
mod clipboard_writer;
mod database;

use clipboard_listener::ClipboardListener;
use database::{AppSettings, ClipboardRecord, Database, ListQuery};
use std::str::FromStr;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

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
fn save_clipboard_text(
    database: State<'_, Database>,
    content: String,
) -> Result<ClipboardRecord, String> {
    database.upsert_text(&content)
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
    database: State<'_, Database>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let updated = database.update_settings(settings)?;
    database.cleanup(&updated)?;
    Ok(updated)
}

#[tauri::command]
fn set_clipboard_listener_paused(listener: State<'_, ClipboardListener>, paused: bool) -> bool {
    listener.set_paused(paused);
    listener.is_paused()
}

#[tauri::command]
fn is_clipboard_listener_paused(listener: State<'_, ClipboardListener>) -> bool {
    listener.is_paused()
}

#[tauri::command]
fn write_clipboard_text(
    listener: State<'_, ClipboardListener>,
    content: String,
) -> Result<(), String> {
    listener.ignore_next_write(&content);
    if let Err(error) = clipboard_writer::write_unicode_text(&content) {
        listener.cancel_ignored_write(&content);
        return Err(error);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkey = tauri_plugin_global_shortcut::Shortcut::from_str("CTRL+SHIFT+V")
        .expect("default global shortcut should be valid");
    let hotkey_id = hotkey.id();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("ClipPaw")
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
            app.manage(listener);
            if let Err(error) = app.global_shortcut().register(hotkey) {
                eprintln!("global shortcut registration failed: {error}");
            }

            let menu = tauri::menu::MenuBuilder::new(app)
                .text("open", "Open Clipboard")
                .text("settings", "Settings")
                .separator()
                .text("pause", "Pause Listening")
                .separator()
                .text("quit", "Quit")
                .build()?;
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
                        listener.set_paused(!listener.is_paused());
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
            tray_builder.build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_clipboard_items,
            save_clipboard_text,
            delete_clipboard_item,
            set_clipboard_item_pinned,
            clear_clipboard_history,
            set_clipboard_listener_paused,
            is_clipboard_listener_paused,
            write_clipboard_text,
            get_app_settings,
            update_app_settings,
            get_autostart_enabled,
            set_autostart_enabled
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
