import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ClipboardList, LoaderCircle, Moon, Pin, Search, Settings2, Sun, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  clearClipboardHistory,
  deleteClipboardItem,
  getAutostartEnabled,
  getAppSettings,
  isClipboardListenerPaused,
  listClipboardItems,
  onClipboardUpdated,
  onListenerPausedChanged,
  setAppLanguage,
  setClipboardItemPinned,
  setClipboardListenerPaused,
  setAutostartEnabled,
  updateAppSettings,
  writeClipboardText,
  type AppSettings,
  type ClipboardRecord,
  type Language,
} from "./lib/clipboard";

const localeOf = (language: Language) => (language === "zh-CN" ? "zh-CN" : "en-US");

function formatTimestamp(timestamp: number, language: Language, todayLabel: string): string {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const locale = localeOf(language);
  if (date.toDateString() === now.toDateString()) {
    const time = date.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
    return `${todayLabel} ${time}`;
  }
  return date.toLocaleDateString(locale, { month: "numeric", day: "numeric" });
}

function sortRecords(records: ClipboardRecord[]): ClipboardRecord[] {
  return [...records].sort((a, b) => Number(b.is_pinned) - Number(a.is_pinned) || b.updated_at - a.updated_at);
}

const labels = {
  "zh-CN": {
    localHistory: "本地剪贴板历史", toggleTheme: "切换主题", openSettings: "打开设置", search: "搜索剪贴板...", searchAria: "搜索剪贴板历史",
    recent: "最近复制", found: (n: number) => `找到 ${n} 条记录`, paused: "监听已暂停", syncing: "同步中...", stored: "仅存储在本机", loading: "正在读取历史...", failed: "读取历史失败", retry: "重试", noMatch: "没有匹配的记录", empty: "还没有剪贴板历史", copyHint: "复制一些文本，它会出现在这里。", copy: "复制", unpin: "取消固定", pin: "固定", delete: "删除", settings: "设置", retention: "历史记录保留策略", max: "最多保留记录", maxHint: "超出后优先清理未固定项目", cleanup: "自动清理天数", cleanupHint: "按最近使用时间计算，0 表示不清理", never: "关闭", pause: "暂停剪贴板监听", pauseHint: "暂停后不会记录新的复制内容", launch: "开机自启动", launchHint: "随 Windows 在后台启动", cancel: "取消", save: "保存", saving: "保存中...", language: "语言", chinese: "简体中文", english: "English", footer: "↑↓ 选择 · Enter 复制 · Esc 关闭", today: "今天", storedLocal: "本地 SQLite", clearAll: "清空历史记录", clearAllHint: "保留置顶项", confirmClear: "确认清空"
  },
  en: {
    localHistory: "Local clipboard history", toggleTheme: "Toggle theme", openSettings: "Open settings", search: "Search clipboard...", searchAria: "Search clipboard history",
    recent: "Recent copies", found: (n: number) => `Found ${n} item(s)`, paused: "Listening paused", syncing: "Syncing...", stored: "Stored locally", loading: "Loading history...", failed: "Failed to load history", retry: "Retry", noMatch: "No matching records", empty: "No clipboard history yet", copyHint: "Copy some text and it will appear here.", copy: "Copy", unpin: "Unpin", pin: "Pin", delete: "Delete", settings: "Settings", retention: "History retention and listener", max: "Maximum records", maxHint: "Unpinned items are cleaned first", cleanup: "Cleanup after", cleanupHint: "Counted from last use, 0 disables", never: "Never", pause: "Pause listener", pauseHint: "Do not capture new copies", launch: "Launch at login", launchHint: "Start in the background with Windows", cancel: "Cancel", save: "Save", saving: "Saving...", language: "Language", chinese: "简体中文", english: "English", footer: "↑↓ Select · Enter Copy · Esc Close", today: "Today", storedLocal: "SQLite local", clearAll: "Clear history", clearAllHint: "Pinned items are kept", confirmClear: "Confirm"
  }
} as const;

function App() {
  const [language, setLanguage] = useState<Language>(() => (localStorage.getItem("clippaw-language") as Language) || "zh-CN");
  const t = labels[language];
  const [query, setQuery] = useState("");
  const [records, setRecords] = useState<ClipboardRecord[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [dark, setDark] = useState(() => {
    const saved = localStorage.getItem("clippaw-theme");
    if (saved === "dark") return true;
    if (saved === "light") return false;
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; record: ClipboardRecord } | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<AppSettings>({ max_history: 1000, cleanup_days: 30, language: "zh-CN" });
  const [draftSettings, setDraftSettings] = useState<AppSettings>({ max_history: 1000, cleanup_days: 30, language: "zh-CN" });
  const [paused, setPaused] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [settingsSaving, setSettingsSaving] = useState(false);
  const [confirmingClear, setConfirmingClear] = useState(false);
  const queryRef = useRef(query);
  const requestId = useRef(0);
  const dialogRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => { queryRef.current = query; }, [query]);
  useEffect(() => { localStorage.setItem("clippaw-language", language); }, [language]);

  const loadRecords = useCallback(async (search: string) => {
    const request = ++requestId.current;
    setLoading(true);
    try {
      const next = await listClipboardItems(search);
      if (request !== requestId.current) return;
      setRecords(sortRecords(next));
      setError(null);
      setSelectedId((current) => next.some((item) => item.id === current) ? current : next[0]?.id ?? null);
    } catch (reason) {
      if (request === requestId.current) setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (request === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => { void loadRecords(query); }, [loadRecords, query]);

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void onClipboardUpdated((record) => {
      if (queryRef.current && !record.content.toLowerCase().includes(queryRef.current.toLowerCase())) {
        setRecords((current) => current.filter((item) => item.id !== record.id));
        return;
      }
      setRecords((current) => sortRecords([...current.filter((item) => item.id !== record.id), record]));
      setSelectedId((current) => current ?? record.id);
    }).then((unlisten) => { cleanup = unlisten; });
    return () => cleanup?.();
  }, []);

  useEffect(() => {
    void Promise.all([getAppSettings(), isClipboardListenerPaused(), getAutostartEnabled()]).then(([loaded, isPaused, startsAutomatically]) => {
      setSettings(loaded); setDraftSettings(loaded); setPaused(isPaused); setAutostart(startsAutomatically);
      // 后端设置是语言的主来源，localStorage 只作为首屏兜底
      setLanguage(loaded.language);
    }).catch((reason) => setActionError(reason instanceof Error ? reason.message : String(reason)));
  }, []);

  // 托盘菜单里切换暂停时，界面要同步
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void onListenerPausedChanged((next) => setPaused(next)).then((unlisten) => { cleanup = unlisten; });
    return () => cleanup?.();
  }, []);

  // 设置弹窗打开时，Esc 关弹窗而不是关窗口
  useEffect(() => {
    if (!showSettings) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setShowSettings(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    dialogRef.current?.focus();
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [showSettings]);

  // 弹窗关闭后重置"确认清空"状态，避免下次打开就是待确认态
  useEffect(() => {
    if (!showSettings) setConfirmingClear(false);
  }, [showSettings]);

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void listen("open-settings", () => { setDraftSettings(settings); setShowSettings(true); }).then((unlisten) => { cleanup = unlisten; });
    return () => cleanup?.();
  }, [settings]);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("clippaw-theme", dark ? "dark" : "light");
  }, [dark]);
  useEffect(() => { const close = () => setContextMenu(null); window.addEventListener("click", close); return () => window.removeEventListener("click", close); }, []);

  const selectedIndex = useMemo(() => records.findIndex((record) => record.id === selectedId), [records, selectedId]);
  function moveSelection(direction: 1 | -1) {
    if (!records.length) return;
    const index = selectedIndex < 0 ? 0 : selectedIndex;
    setSelectedId(records[(index + direction + records.length) % records.length].id);
  }
  async function copySelected() {
    const selected = records.find((record) => record.id === selectedId);
    if (!selected) return;
    try { setActionError(null); await writeClipboardText(selected.content); await getCurrentWindow().hide(); }
    catch (reason) { setActionError(reason instanceof Error ? reason.message : String(reason)); }
  }
  async function deleteRecord(record: ClipboardRecord) {
    try { setActionError(null); await deleteClipboardItem(record.id); setRecords((current) => current.filter((item) => item.id !== record.id)); setSelectedId((current) => current === record.id ? null : current); setContextMenu(null); }
    catch (reason) { setActionError(reason instanceof Error ? reason.message : String(reason)); }
  }
  async function togglePinned(record: ClipboardRecord) {
    try { const pinned = !record.is_pinned; await setClipboardItemPinned(record.id, pinned); setRecords((current) => sortRecords(current.map((item) => item.id === record.id ? { ...item, is_pinned: pinned } : item))); setContextMenu(null); }
    catch (reason) { setActionError(reason instanceof Error ? reason.message : String(reason)); }
  }
  async function saveSettings() {
    setSettingsSaving(true);
    setActionError(null);
    try { const updated = await updateAppSettings({ ...draftSettings, language }); const actualPaused = await setClipboardListenerPaused(paused); const actualAutostart = await setAutostartEnabled(autostart); setSettings(updated); setDraftSettings(updated); setPaused(actualPaused); setAutostart(actualAutostart); setShowSettings(false); await loadRecords(query); }
    catch (reason) { setActionError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setSettingsSaving(false); }
  }
  async function clearHistory() {
    if (!confirmingClear) { setConfirmingClear(true); return; }
    try {
      setActionError(null);
      await clearClipboardHistory();
      setConfirmingClear(false);
      setShowSettings(false);
      await loadRecords(query);
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : String(reason));
      setConfirmingClear(false);
    }
  }

  return <main className="min-h-screen bg-background text-foreground antialiased">
    <section className="mx-auto flex min-h-screen w-full max-w-2xl flex-col px-4 py-5 sm:px-6">
      <header className="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-border/70 bg-card/70 px-3 py-2.5 shadow-sm">
        <div className="flex items-center gap-2.5"><span className="flex size-9 items-center justify-center overflow-hidden rounded-xl border border-border/80 bg-muted shadow-sm"><img src="/icons/icon.png" alt="ClipPaw" className="size-full object-cover" /></span><div><p className="text-sm font-semibold tracking-tight text-foreground">ClipPaw</p><p className="text-xs text-muted-foreground">{t.localHistory}</p></div></div>
        <div className="flex items-center gap-1"><button className="icon-button" type="button" aria-label={t.toggleTheme} onClick={() => setDark((value) => !value)}>{dark ? <Sun className="size-4" /> : <Moon className="size-4" />}</button><button className="icon-button" type="button" aria-label={t.openSettings} onClick={() => { setDraftSettings(settings); setShowSettings(true); }}><Settings2 className="size-4" /></button></div>
      </header>
      <div className="relative mb-3"><Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" /><input autoFocus value={query} onChange={(event) => setQuery(event.currentTarget.value)} onKeyDown={(event) => { if (event.key === "ArrowDown") { event.preventDefault(); moveSelection(1); } if (event.key === "ArrowUp") { event.preventDefault(); moveSelection(-1); } if (event.key === "Enter") { event.preventDefault(); void copySelected(); } if (event.key === "Delete") { event.preventDefault(); const selected = records.find((record) => record.id === selectedId); if (selected) void deleteRecord(selected); } if (event.key === "Escape") { event.preventDefault(); void getCurrentWindow().hide(); } }} className="h-11 w-full rounded-xl border border-border bg-card pl-10 pr-20 text-sm shadow-sm outline-none transition placeholder:text-muted-foreground focus:border-ring focus:ring-2 focus:ring-ring/20" placeholder={t.search} aria-label={t.searchAria} /><kbd className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 rounded-md border border-border bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Ctrl F</kbd></div>
      <div className="mb-2 flex items-center justify-between px-1 text-xs text-muted-foreground"><span>{query ? t.found(records.length) : t.recent}</span><span>{paused ? t.paused : loading ? t.syncing : t.stored}</span></div>
      {actionError && <div className="mb-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">{actionError}</div>}
      <div className="flex-1 overflow-hidden rounded-2xl border border-border bg-card shadow-sm">{loading && !records.length ? <div className="flex min-h-48 flex-col items-center justify-center gap-2 text-sm text-muted-foreground"><LoaderCircle className="size-5 animate-spin" /><span>{t.loading}</span></div> : error ? <div className="flex min-h-48 flex-col items-center justify-center gap-2 px-6 text-center"><p className="text-sm font-medium">{t.failed}</p><p className="text-xs text-muted-foreground">{error}</p><button className="text-xs font-medium text-primary hover:underline" type="button" onClick={() => void loadRecords(query)}>{t.retry}</button></div> : records.length ? <div className="divide-y divide-border/70">{records.map((record) => { const selected = selectedId === record.id; return <button key={record.id} type="button" className={`group flex w-full items-start gap-3 px-4 py-3.5 text-left transition ${selected ? "bg-accent" : "hover:bg-accent/60"}`} onClick={() => setSelectedId(record.id)} onDoubleClick={() => void copySelected()} onContextMenu={(event) => { event.preventDefault(); setSelectedId(record.id); const menuWidth = 176; const menuHeight = 132; setContextMenu({ x: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth)), y: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight)), record }); }}><span className={`mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-lg ${selected ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground"}`}>{record.is_pinned ? <Pin className="size-3.5" /> : <ClipboardList className="size-3.5" />}</span><span className="min-w-0 flex-1"><span className="block whitespace-pre-wrap break-words text-sm font-medium leading-6">{record.content}</span><span className="block text-xs text-muted-foreground">{formatTimestamp(record.updated_at, language, t.today)}</span></span></button>; })}</div> : <div className="flex min-h-48 flex-col items-center justify-center gap-2 text-center"><Search className="size-5 text-muted-foreground" /><p className="text-sm font-medium">{query ? t.noMatch : t.empty}</p><p className="text-xs text-muted-foreground">{t.copyHint}</p></div>}</div>
      {contextMenu && <div className="fixed z-50 min-w-36 overflow-hidden rounded-xl border border-border bg-card p-1 text-xs shadow-lg" style={{ left: contextMenu.x, top: contextMenu.y }} onClick={(event) => event.stopPropagation()}><button className="context-menu-item" type="button" onClick={() => void copySelected()}>{t.copy}</button><button className="context-menu-item" type="button" onClick={() => void togglePinned(contextMenu.record)}>{contextMenu.record.is_pinned ? t.unpin : t.pin}</button><button className="context-menu-item text-destructive" type="button" onClick={() => void deleteRecord(contextMenu.record)}>{t.delete}</button></div>}
      {showSettings && <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/20 p-4 backdrop-blur-[2px]" onClick={() => setShowSettings(false)}><div ref={dialogRef} tabIndex={-1} className="w-full max-w-sm rounded-2xl border border-border bg-card p-5 shadow-xl outline-none" role="dialog" aria-modal="true" onClick={(event) => event.stopPropagation()}><div className="mb-4 flex items-center justify-between"><div><h2 className="text-base font-semibold">{t.settings}</h2><p className="text-xs text-muted-foreground">{t.retention}</p></div><button className="icon-button" type="button" aria-label={t.cancel} onClick={() => setShowSettings(false)}><X className="size-4" /></button></div><div className="space-y-4 text-sm"><label className="flex items-center justify-between gap-4"><span className="block font-medium">{t.language}</span><select className="h-9 rounded-lg border border-border bg-background px-2 text-sm" value={language} onChange={(event) => { const next = event.target.value as Language; setLanguage(next); setDraftSettings((current) => ({ ...current, language: next })); void setAppLanguage(next); }}><option value="zh-CN">{t.chinese}</option><option value="en">{t.english}</option></select></label><label className="flex items-center justify-between gap-4"><span><span className="block font-medium">{t.max}</span><span className="text-xs text-muted-foreground">{t.maxHint}</span></span><select className="h-9 rounded-lg border border-border bg-background px-2 text-sm" value={draftSettings.max_history} onChange={(event) => setDraftSettings((current) => ({ ...current, max_history: Number(event.target.value) }))}>{[100, 500, 1000, 5000, 10000].map((value) => <option key={value} value={value}>{value}</option>)}</select></label><label className="flex items-center justify-between gap-4"><span><span className="block font-medium">{t.cleanup}</span><span className="text-xs text-muted-foreground">{t.cleanupHint}</span></span><select className="h-9 rounded-lg border border-border bg-background px-2 text-sm" value={draftSettings.cleanup_days} onChange={(event) => setDraftSettings((current) => ({ ...current, cleanup_days: Number(event.target.value) }))}>{[0, 1, 7, 30].map((value) => <option key={value} value={value}>{value === 0 ? t.never : `${value} ${language === "zh-CN" ? "天" : "days"}`}</option>)}</select></label><label className="flex items-center justify-between gap-4 rounded-xl border border-border/70 p-3"><span><span className="block font-medium">{t.pause}</span><span className="text-xs text-muted-foreground">{t.pauseHint}</span></span><input type="checkbox" className="size-4 accent-primary" checked={paused} onChange={(event) => setPaused(event.target.checked)} /></label><label className="flex items-center justify-between gap-4 rounded-xl border border-border/70 p-3"><span><span className="block font-medium">{t.launch}</span><span className="text-xs text-muted-foreground">{t.launchHint}</span></span><input type="checkbox" className="size-4 accent-primary" checked={autostart} onChange={(event) => setAutostart(event.target.checked)} /></label><div className="flex items-center justify-between gap-4 rounded-xl border border-destructive/30 p-3"><span><span className="block font-medium text-destructive">{t.clearAll}</span><span className="text-xs text-muted-foreground">{t.clearAllHint}</span></span><button type="button" className="shrink-0 rounded-lg border border-destructive/40 px-3 py-1.5 text-xs font-medium text-destructive transition hover:bg-destructive/10" onClick={() => void clearHistory()}>{confirmingClear ? t.confirmClear : t.clearAll}</button></div></div><div className="mt-5 flex justify-end gap-2"><button className="rounded-lg px-3 py-2 text-sm text-muted-foreground hover:bg-accent" type="button" onClick={() => setShowSettings(false)}>{t.cancel}</button><button className="rounded-lg bg-primary px-3 py-2 text-sm text-primary-foreground disabled:opacity-50" type="button" disabled={settingsSaving} onClick={() => void saveSettings()}>{settingsSaving ? t.saving : t.save}</button></div></div></div>}
      <footer className="mt-3 flex items-center justify-between px-1 text-xs text-muted-foreground"><span>{t.footer}</span><span>{t.storedLocal}</span></footer>
    </section>
  </main>;
}

export default App;
