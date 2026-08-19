# dsh-desktop

把 DeepSeek Harness（dsh）的 web 端包成 Windows 桌面客户端的壳：**Tauri 2（Rust）+ WebView2**，双击即用，安装包约几 MB。

```
┌────────────────────────────────────────────┐
│  dsh-desktop.exe (Tauri 2 / WebView2)      │
│                                            │
│  ┌──────────────┐   拉起/复用   ┌────────┐ │
│  │ 窗口(本地 splash)│ ──────────▶ │ dsh web│ │
│  │   ↓ 探活后导航 │   dsh.cmd   │ :3080  │ │
│  │ http://127.0.0.1:3080  ◀──────┴────────┘ │
│  └──────────────┘  退出时 taskkill /T 清树  │
└────────────────────────────────────────────┘
```

## 工作原理

1. 窗口先显示本地静态 splash（`dist/index.html`，"正在启动…"）。
2. 后台线程探测 `127.0.0.1:3080`：
   - **已有 dsh 在服务**（GET `/` 响应里带 `<title>DeepSeek Harness</title>` 标记）→ 直接挂接，退出时**不杀**它；
   - **端口被别的程序占用** → 等 10 秒看它是不是正在启动的 dsh，否则弹错误框；
   - **端口空闲** → `cmd /C dsh web --port <port>`（`CREATE_NO_WINDOW`，不闪黑窗），stdout/stderr 重定向到日志，轮询探活，就绪后 `navigate` 过去。退出时 `taskkill /PID <pid> /T /F` 清理整棵进程树。
3. 单实例：重复启动只聚焦已有窗口。
4. 窗口内导航到任何非 localhost 地址（含 `target=_blank` / `window.open`）一律转交系统默认浏览器。
5. dsh 的页面是远程源（`http://127.0.0.1:3080`），Tauri 的 capabilities 只授予本地页面，**dsh 前端拿不到任何 Tauri IPC**，也不给它 `withGlobalTauri`。

## 目录结构

```
dsh-desktop/
├── dist/index.html            # splash 页（唯一的前端资源）
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

日志：`%LOCALAPPDATA%\com.dsh.desktop\logs\dsh-server.log`（`app_log_dir` 按 identifier 解析）。

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
| 编译报错 `on_webview_event` 不存在 | tauri 版本过旧，升级：`cargo update -p tauri`（需 ≥2.2） |
| 白屏 | 确认 3080 有 dsh 在响应；dev 模式打开 devtools 看控制台 |

## 路线图（未实现）

- 系统托盘 / 最小化到托盘 / 开机自启（`tauri-plugin-autostart`）
- 自动更新（`tauri-plugin-updater` + 自建静态签名服务）
- 把 node + `@deepseek-ai/dsh` 作为 Tauri sidecar 一起打包，做到真正的免依赖分发
- macOS/Linux 适配（`lib.rs` 里已留 `cfg` 分支：`open`/`xdg-open`/普通 `kill`）
