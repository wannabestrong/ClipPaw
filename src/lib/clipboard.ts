import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ClipboardRecord = {
  id: number;
  content: string;
  content_type: string;
  created_at: number;
  updated_at: number;
  is_pinned: boolean;
  hash: string;
};

export type AppSettings = {
  max_history: number;
  cleanup_days: number;
};

export async function listClipboardItems(search: string): Promise<ClipboardRecord[]> {
  return invoke<ClipboardRecord[]>("list_clipboard_items", {
    query: { search: search || null, limit: 1000 },
  });
}

export function onClipboardUpdated(
  callback: (record: ClipboardRecord) => void,
): Promise<UnlistenFn> {
  return listen<ClipboardRecord>("clipboard-updated", (event) => {
    callback(event.payload);
  });
}

export function writeClipboardText(content: string): Promise<void> {
  return invoke("write_clipboard_text", { content });
}

export function deleteClipboardItem(id: number): Promise<boolean> {
  return invoke<boolean>("delete_clipboard_item", { id });
}

export function setClipboardItemPinned(id: number, pinned: boolean): Promise<boolean> {
  return invoke<boolean>("set_clipboard_item_pinned", { id, pinned });
}

export function getAppSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_app_settings");
}

export function updateAppSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>("update_app_settings", { settings });
}

export function setClipboardListenerPaused(paused: boolean): Promise<boolean> {
  return invoke<boolean>("set_clipboard_listener_paused", { paused });
}

export function isClipboardListenerPaused(): Promise<boolean> {
  return invoke<boolean>("is_clipboard_listener_paused");
}

export function getAutostartEnabled(): Promise<boolean> {
  return invoke<boolean>("get_autostart_enabled");
}

export function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart_enabled", { enabled });
}
