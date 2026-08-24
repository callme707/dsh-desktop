# dsh-desktop

把 DeepSeek Harness（dsh）的 web 端包成 Windows 桌面客户端的壳：**Tauri 2（Rust）+ WebView2**，双击即用，安装包约几 MB。

```
┌────────────────────────────────────────────────────┐
│  无边框 Tauri 窗口                                 │
│  ┌──────────────────────────────────────────────┐  │
│  │ chrome WebView：本地可信顶栏 / 窗口与更新操作 │  │
│  ├──────────────────────────────────────────────┤  │
│  │ content WebView：splash / 更新页 / dsh Web UI │  │
│  └───────────────────────────────┬──────────────┘  │
│                    拉起/复用     │                 │
│                    dsh.cmd  ┌────▼────┐            │
│                             │ dsh web │ :3080      │
│                             └─────────┘            │
└────────────────────────────────────────────────────┘
```

## 工作原理

1. 一个无边框原生窗口承载两个隔离的 WebView：`chrome` 始终显示本地可信顶栏，`content` 先显示本地静态 splash（`dist/index.html`，"正在启动…"）。
2. 后台线程探测 `127.0.0.1:3080`：
   - **已有 dsh 在服务**（GET `/` 响应里带 `<title>DeepSeek Harness</title>` 标记）→ 直接挂接，退出时**不杀**它；
   - **端口被别的程序占用** → 等 10 秒看它是不是正在启动的 dsh，否则弹错误框；
   - **端口空闲** → `cmd /C dsh web --no-open --port <port>`（`CREATE_NO_WINDOW`，不闪黑窗，也不额外打开系统浏览器），stdout/stderr 重定向到日志，轮询探活，就绪后 `navigate` 过去。退出时 `taskkill /PID <pid> /T /F` 清理整棵进程树。
3. 单实例：重复启动只聚焦已有窗口。
4. 窗口内导航到任何非 localhost 地址（含 `target=_blank` / `window.open`）一律转交系统默认浏览器。
5. dsh 的页面是远程源（`http://127.0.0.1:3080`），Tauri capabilities 只授予 `chrome` WebView；**dsh 前端拿不到任何 Tauri IPC**，也不给它 `withGlobalTauri`。
6. **dsh 本体自动更新**：持久顶栏带「检查 dsh 更新」按钮，启动时也会静默检查。发现新版 → 弹窗确认 → `content` 切到本地 `update.html` 更新页（dsh 界面被替换，更新过程可见、页面不可操作）→ 停掉自拉的 dsh → 后台 `npm i -g @deepseek-ai/dsh@latest`（日志落盘）→ 完成/失败弹窗 → 确认后自动重启应用收尾。

## 目录结构

```
dsh-desktop/
├── dist/chrome.html           # 持久自定义顶栏
├── dist/chrome.css            # 顶栏布局、状态与窗口按钮样式
├── dist/chrome.js             # 窗口操作、更新入口和状态同步
├── dist/index.html            # 启动链路页
├── dist/update.html           # dsh 更新链路页
├── dist/shell.css             # 两个本地页面共用的视觉系统
├── dist/assets/               # 应用图标与 Tabler Icons（MIT）
├── scripts/gen-icons.mjs      # 占位图标生成器（纯 Node，无依赖）
├── package.json               # 只是 tauri CLI 的载体（@tauri-apps/cli）
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json        # 产品名/identifier/NSIS 配置
    ├── capabilities/default.json
    ├── icons/                 # 生成产物（正式发布前换真实 logo）
    └── src/
        ├── main.rs            # windows_subsystem="windows"
        └── lib.rs             # 全部逻辑：进程管理/探活/导航/外链
```

## 前置条件

| 依赖 | 说明 |
| --- | --- |
| Rust（MSVC 工具链） | [rustup](https://rustup.rs/) 默认安装，另需 VS Build Tools 的「使用 C++ 的桌面开发」 |
| Node.js | 已确认本机 `C:\Program Files\nodejs` 可用 |
| dsh（npm 全局） | `npm i -g @deepseek-ai/dsh`，`dsh` 需在 PATH 上（本机已装） |
| WebView2 Runtime | Win10/11 基本自带；发布包默认 `downloadBootstrapper`，缺了会自动引导安装 |

## 构建

```powershell
cd dsh-desktop
npm install            # 只装 @tauri-apps/cli
npm run icons          # 重新生成占位图标（可跳过，仓库里已带）
npm run dev            # 开发运行（有 devtools，ctrl+shift+i）
npm run build          # 正式构建
# 产物：src-tauri/target/release/dsh-desktop.exe
# 安装包：src-tauri/target/release/bundle/nsis/dsh-desktop_0.1.0_x64-setup.exe
```

首次编译要拉约 400 个 crate，耐心等几分钟。

## 配置

| 变量 | 作用 | 默认 |
| --- | --- | --- |
| `DSH_WEB_PORT` | dsh web 端口（同时也是探活端口） | `3080`（dsh-web-app 默认值） |
| `DSH_HOME` | dsh 的工作目录/配置根 | 不设则用当前目录 |
| `DSH_NO_UPDATE_CHECK` | 设为任意值跳过启动时的 dsh 更新检查 | 不设（自动检查） |

日志：`%LOCALAPPDATA%\com.dsh.desktop\logs\dsh-server.log`（`app_log_dir` 按 identifier 解析），更新日志同目录 `dsh-update.log`。

## dsh 自动更新

- 入口两个：dsh 就绪后启用持久顶栏的刷新按钮（手动触发，任何结果都有弹窗反馈）+ 启动时静默检查（`DSH_NO_UPDATE_CHECK=1` 只关后者，不关手动入口）
- 版本检查走 `npm view`（自动继承你 `.npmrc` 里的代理/镜像配置，国内环境无需额外处理）；已安装版本优先从 `npm root -g` 定位，`%APPDATA%\npm` 仅作兜底，版本比较使用标准 SemVer（含预发布版本）
- 确认更新后主窗口切回 Tauri 内置的 `update.html` 四阶段更新页，dsh 界面被替换、不可再操作；窗口标题变为「dsh 更新中…」
- 更新前会先 `taskkill /T` 停掉**本客户端拉起**的 dsh（避免 Windows 文件占用）；你终端里手动跑的 dsh 不受影响，但也要手动重启才用上新版本
- npm 返回成功后还会重新读取全局包版本，只有实际版本达到目标才算更新成功
- 完成/失败都会弹单按钮对话框，确认后自动重启应用收尾；更新任务进行中刷新按钮会禁用，即使从 IPC 重复触发也会被后端状态锁拒绝
- 更新失败/超时（10 分钟）弹窗会指向 `dsh-update.log`

## 国内网络加速（可选）

crates.io 直连慢的话，在 `%USERPROFILE%\.cargo\config.toml` 加：

```toml
[source.crates-io]
replace-with = 'rsproxy-sparse'

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
```

npm 慢则 `npm install --registry=https://registry.npmmirror.com`。

## 发布前必做

1. **换图标**：拿一张 ≥1024px 方形 PNG 跑 `npx tauri icon logo.png`，自动覆盖 `src-tauri/icons`。
2. **改名字**：`tauri.conf.json` 的 `productName`、`identifier`，`package.json` 的 `name`。
3. **代码签名**：不签名的话 SmartScreen 会报"未知发布者"（个人 OV 证书即可消除大部分提示；企业内部可选免费策略）。
4. 想改成"关闭窗口 = 最小化到托盘 + dsh 后台常驻"的话，加 `tauri-plugin-tray` 并拦 `CloseRequested` 即可，代码结构已留好扩展点。

## 故障排查

| 现象 | 处理 |
| --- | --- |
| 弹窗「dsh 进程提前退出」 | `npm i -g @deepseek-ai/dsh`，确认 `where dsh` 有结果；看日志文件 |
| 弹窗「端口被占用」 | 设 `DSH_WEB_PORT=3081` 后重启客户端 |
| 弹窗「等待超时」 | 看日志；首次启动会引导 profile（写 `~/.dsh/profiles/web/cordis.yml`），一般 <15s |
| dsh 更新失败 | 看 `%LOCALAPPDATA%\com.dsh.desktop\logs\dsh-update.log`；常见原因是代理没开或 npm registry 连不上 |
| 编译报错 `on_webview_event` 不存在 | tauri 版本过旧，升级：`cargo update -p tauri`（需 ≥2.2） |
| 白屏 | 确认 3080 有 dsh 在响应；dev 模式打开 devtools 看控制台 |

## 路线图（未实现）

- 系统托盘 / 最小化到托盘 / 开机自启（`tauri-plugin-autostart`）
- 壳自身的自动更新（`tauri-plugin-updater` + 静态签名服务）——目前只实现了 dsh 本体的更新
- 把 node + `@deepseek-ai/dsh` 作为 Tauri sidecar 一起打包，做到真正的免依赖分发
- macOS/Linux 适配（`lib.rs` 里已留 `cfg` 分支：`open`/`xdg-open`/普通 `kill`）
