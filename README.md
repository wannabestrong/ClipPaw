# ClipPaw

轻量、快速、隐私优先的 Windows 剪贴板历史管理器。

文本监听、搜索、置顶、删除、全局快捷键、系统托盘、开机自启、自动清理、中英文界面。
所有历史记录只保存在本机 SQLite 数据库，不上传任何内容。

## 功能

- **自动记录**：监听系统剪贴板，重复内容只更新时间戳，不会堆出重复行
- **快速唤起**：`Ctrl+Shift+V` 呼出窗口，`↑` `↓` 选择、`Enter` 复制、`Esc` 关闭、`Delete` 删除
- **搜索**：按内容即时过滤
- **置顶**：常用内容固定在最前，且不会被自动清理删除
- **托盘常驻**：关闭窗口只是隐藏，托盘菜单可打开、暂停监听、退出
- **开机自启**：登录时静默启动，只驻留托盘，不弹窗
- **隐私**：尊重 Windows 的"排除剪贴板历史"标记，密码管理器写入的内容不会被记录
- **自动清理**：按最近使用时间或条数上限清理，置顶项始终保留
- **中英文界面**：语言随设置保存，托盘菜单同步切换

## 数据位置

```
%APPDATA%\com.wannabestrong.clippaw\clipboard-history.sqlite3
```

删除该目录即可重置所有历史与设置。

## 开发

需要 Node.js 20+、pnpm 9+ 和 Rust 工具链。

```bash
pnpm install
pnpm tauri dev
```

## 构建

```bash
pnpm build          # 只构建前端
pnpm tauri build    # 构建 exe 与安装包
```

### 关于 Windows 工具链

推荐使用 **MSVC** 工具链（`rustup default stable-x86_64-pc-windows-msvc`），
需要安装 Visual Studio Build Tools 的 C++ 生成工具。

如果只能用 GNU/MinGW 工具链（例如没有管理员权限）：

1. 安装 MinGW-w64，并确保 `gcc`、`windres` 在 `PATH` 中
2. `cargo build --release` 可以正常产出 exe
3. **必须把 `WebView2Loader.dll` 和 exe 放在同一目录**——GNU 工具链无法像 MSVC 那样
   把 WebView2 loader 静态链接进 exe。该 DLL 来自
   `~/.cargo/registry/src/*/webview2-com-sys-*/x64/`
4. 链接时会出现 `.rsrc merge failure: multiple non-default manifests` 警告，
   不影响运行，但高 DPI 下的缩放表现可能与 MSVC 构建有差异

## 发布

推送 `v*` 形式的 tag 会触发 `Release` workflow，在 GitHub 的 Windows runner 上自动
构建 NSIS 安装包和 MSI，并作为附件挂到对应的 Release 上：

```bash
git tag v1.2.0
git push origin v1.2.0
```

本地 `pnpm tauri build` 会顺带生成便携版目录
`src-tauri/target/release/clippaw.exe`（GNU 工具链下记得把它和
`WebView2Loader.dll` 放一起）。

## 已知限制

- 仅支持 Windows（剪贴板监听依赖 Win32 `AddClipboardFormatListener`）
- 只记录纯文本，图片和文件暂不支持
- 全局快捷键目前固定为 `Ctrl+Shift+V`，不可配置

## 许可

MIT
