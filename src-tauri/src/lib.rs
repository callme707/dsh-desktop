use std::{
    fs::File,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use semver::Version;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Rect, RunEvent, Theme, Webview,
    WebviewBuilder, WebviewUrl, Window, WindowBuilder, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// dsh web 端默认端口（dsh-web-app 的 cordis.patch.yml：`port: !!js ctx.webStartup.port ?? 3080`）
const DEFAULT_PORT: u16 = 3080;
/// 等待 dsh 服务就绪的最长时间
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// dsh 前端 <title> 标记，用于区分“3080 上真的是 dsh”与“被别的程序占了”
const DSH_MARKER: &str = "DeepSeek Harness";
const DSH_PACKAGE: &str = "@deepseek-ai/dsh";
const DSH_LATEST_PACKAGE: &str = "@deepseek-ai/dsh@latest";
const NPM_QUERY_TIMEOUT: Duration = Duration::from_secs(60);
const NPM_UPDATE_TIMEOUT: Duration = Duration::from_secs(600);
const CHROME_HEIGHT: f64 = 52.0;
const CHROME_WEBVIEW_LABEL: &str = "chrome";
const CONTENT_WEBVIEW_LABEL: &str = "content";

/// 由本客户端拉起的 dsh 子进程（挂接到已存在服务时保持 None，退出时不杀它）
#[derive(Default)]
struct ServerState(Mutex<Option<Child>>);

/// 更新任务进行中标记（防止顶栏按钮连点/启动检查叠加）
#[derive(Default)]
struct UpdateState(Mutex<bool>);

/// 启动时从本地窗口捕获的 update.html URL；窗口后来导航到 dsh 后仍可安全返回本地资源。
#[derive(Default)]
struct UpdatePageState(Mutex<Option<tauri::Url>>);

#[derive(Clone, Copy, Default)]
enum KernelStatus {
    #[default]
    Starting,
    Ready,
    Checking,
    Available,
    Updating,
}

impl KernelStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Checking => "checking",
            Self::Available => "available",
            Self::Updating => "updating",
        }
    }
}

#[derive(Default)]
struct ChromeState(Mutex<KernelStatus>);

fn resolve_port() -> u16 {
    std::env::var("DSH_WEB_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

fn is_local_url(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("127.0.0.1") | Some("localhost") | Some("tauri.localhost")
    )
}

/// TCP 探测：端口上是否已有监听（不校验是谁）
fn tcp_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

/// GET / 并校验响应里带 dsh 首页标记，避免把 3080 上的无关应用当成 dsh
fn dsh_serving(port: u16) -> bool {
    let mut stream = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
    let request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).is_err() {
        return false;
    }
    String::from_utf8_lossy(&buf).contains(DSH_MARKER)
}

/// 通用 spawn：建日志目录、重定向 stdio、Windows 下不弹黑窗
fn spawn_with(log_dir: &Path, program: &str, args: &[String]) -> std::io::Result<Child> {
    // 关键：File::create 不会自动建父目录，logs 目录首次不存在时会导致启动失败
    std::fs::create_dir_all(log_dir)?;
    let log = File::create(log_dir.join("dsh-server.log"))?;
    let err = log.try_clone()?;
    let home = std::env::var("DSH_HOME").unwrap_or_else(|_| ".".into());

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err))
        .current_dir(home);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn()
}

/// Web profile 只提供嵌入式内容，禁止 dsh 额外打开系统默认浏览器。
fn dsh_web_args(port: u16) -> Vec<String> {
    vec![
        "web".into(),
        "--no-open".into(),
        "--port".into(),
        port.to_string(),
    ]
}

/// 主路径：`cmd /C dsh web --no-open --port <port>`（依赖 PATH 里的 dsh）
fn spawn_dsh(port: u16, log_dir: &Path) -> std::io::Result<Child> {
    let command_line = format!("dsh {}", dsh_web_args(port).join(" "));
    spawn_with(log_dir, "cmd", &["/C".into(), command_line])
}

/// 兜底：直接用 node 跑 npm 全局安装的 dsh bin.js（防 GUI 进程 PATH 里没有 dsh.cmd）
fn spawn_dsh_node(port: u16, log_dir: &Path) -> Option<std::io::Result<Child>> {
    let bin = dsh_package_manifest_candidates()
        .into_iter()
        .filter_map(|manifest| {
            manifest
                .parent()
                .map(|package_dir| package_dir.join("lib/bin.js"))
        })
        .find(|candidate| candidate.exists())?;
    let mut args = vec![bin.display().to_string()];
    args.extend(dsh_web_args(port));
    Some(spawn_with(log_dir, "node", &args))
}

/// 轮询等 dsh 就绪；若子进程提前退出（找不到 dsh / 端口冲突 / 配置错误）立刻报错
fn wait_for_dsh(port: u16, child: &mut Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if dsh_serving(port) {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "dsh 进程提前退出（{status}）。请确认已全局安装 dsh（npm i -g @deepseek-ai/dsh）且 PATH 可用"
            ));
        }
        if Instant::now() > deadline {
            return Err(format!("等待 dsh 就绪超时（{} 秒）", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(300));
    }
}

/// 结束整棵进程树（Windows 用 taskkill /T /F）
fn kill_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
}

/// 若子进程仍在运行，结束整棵进程树，并等待句柄完成回收。
fn terminate_child(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(Some(_))) {
        kill_tree(child.id());
    }
    let _ = child.wait();
}

/// 用系统默认浏览器打开外链
fn open_external(url: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("cmd")
            .arg("/C")
            .arg(format!("start \"\" \"{url}\""))
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

/// 弹出错误对话框后退出
fn fail(app: &AppHandle, message: String) {
    app.dialog()
        .message(message)
        .title("dsh-desktop")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    app.exit(1);
}

// ── dsh 本体（npm 包）更新 ────────────────────────────────────────────

fn npm_command(args: &[&str]) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let command_line = format!("npm {}", args.join(" "));
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/S", "/C", &command_line])
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = Command::new("npm");
        cmd.args(args);
        cmd
    }
}

/// 执行输出量很小的 npm 查询命令，并提供显式超时与进程树清理。
fn run_npm_capture(args: &[&str], timeout: Duration) -> Result<String, String> {
    let display_command = format!("npm {}", args.join(" "));
    let mut cmd = npm_command(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("无法启动 {display_command}：{error}"))?;
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(format!("{display_command} 执行失败（{status}）"));
                }
                let mut output = String::new();
                child
                    .stdout
                    .take()
                    .ok_or_else(|| format!("无法读取 {display_command} 的输出"))?
                    .read_to_string(&mut output)
                    .map_err(|error| format!("读取 {display_command} 输出失败：{error}"))?;
                return Ok(output);
            }
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child);
                return Err(format!("等待 {display_command} 失败：{error}"));
            }
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            return Err(format!(
                "{display_command} 超时（{} 秒）",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn dsh_package_manifest_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(output) = run_npm_capture(&["root", "-g"], NPM_QUERY_TIMEOUT) {
        if let Some(root) = output.lines().map(str::trim).find(|line| !line.is_empty()) {
            candidates.push(PathBuf::from(root).join(DSH_PACKAGE).join("package.json"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let fallback = PathBuf::from(appdata)
            .join("npm/node_modules")
            .join(DSH_PACKAGE)
            .join("package.json");
        if !candidates.contains(&fallback) {
            candidates.push(fallback);
        }
    }
    candidates
}

fn parse_dsh_version(raw: &str) -> Result<Version, String> {
    let normalized = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches('v');
    Version::parse(normalized).map_err(|error| format!("无效的 dsh 版本号“{raw}”：{error}"))
}

/// 从 npm 实际全局根目录读取 dsh 的已安装版本；兼容默认 APPDATA 路径作为兜底。
fn installed_dsh_version() -> Result<Version, String> {
    let candidates = dsh_package_manifest_candidates();
    for manifest in &candidates {
        if !manifest.exists() {
            continue;
        }
        let text = std::fs::read_to_string(manifest)
            .map_err(|error| format!("读取 {} 失败：{error}", manifest.display()))?;
        let package: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("解析 {} 失败：{error}", manifest.display()))?;
        let raw = package
            .get("version")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{} 缺少 version 字段", manifest.display()))?;
        return parse_dsh_version(raw);
    }
    Err("未找到全局安装的 @deepseek-ai/dsh（已检查 npm root -g 与 APPDATA 兜底路径）".into())
}

/// 查询 npm registry 上 dsh 的最新版本。
fn latest_dsh_version() -> Result<Version, String> {
    let output = run_npm_capture(&["view", DSH_PACKAGE, "version"], NPM_QUERY_TIMEOUT)?;
    output
        .lines()
        .find_map(|line| parse_dsh_version(line).ok())
        .ok_or_else(|| "npm 返回结果里没有有效的 dsh 版本号".into())
}

fn update_available(latest: &Version, installed: &Version) -> bool {
    latest > installed
}

/// 后台执行 `npm i -g @deepseek-ai/dsh@latest`，并验证实际安装版本。
fn run_dsh_update(log_dir: &Path, expected_version: &Version) -> Result<Version, String> {
    std::fs::create_dir_all(log_dir).map_err(|e| e.to_string())?;
    let log = File::create(log_dir.join("dsh-update.log")).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = npm_command(&["install", "-g", DSH_LATEST_PACKAGE]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("无法启动 npm 更新：{error}"))?;
    let deadline = Instant::now() + NPM_UPDATE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let actual_version = installed_dsh_version()
                    .map_err(|reason| format!("npm 已完成，但无法验证安装版本：{reason}"))?;
                if actual_version < *expected_version {
                    return Err(format!(
                        "npm 已完成，但实际版本仍为 v{actual_version}（期望至少 v{expected_version}）"
                    ));
                }
                return Ok(actual_version);
            }
            Ok(Some(status)) => {
                return Err(format!(
                    "npm 退出码 {status}，详见 {}\\dsh-update.log",
                    log_dir.display()
                ));
            }
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child);
                return Err(format!("等待 npm 更新进程失败：{error}"));
            }
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            return Err(format!(
                "更新超时（10 分钟），详见 {}\\dsh-update.log",
                log_dir.display()
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn set_kernel_status(app: &AppHandle, status: KernelStatus) {
    *app.state::<ChromeState>().0.lock().unwrap() = status;
    if let Some(chrome) = app.get_webview(CHROME_WEBVIEW_LABEL) {
        let script = match status {
            KernelStatus::Starting => "window.setKernelState?.('starting');",
            KernelStatus::Ready => "window.setKernelState?.('ready');",
            KernelStatus::Checking => "window.setKernelState?.('checking');",
            KernelStatus::Available => "window.setKernelState?.('available');",
            KernelStatus::Updating => "window.setKernelState?.('updating');",
        };
        let _ = chrome.eval(script);
    }
}

fn sync_chrome_window_state(window: &Window, chrome: &Webview) {
    let script = if window.is_maximized().unwrap_or(false) {
        "window.setMaximized?.(true);"
    } else {
        "window.setMaximized?.(false);"
    };
    let _ = chrome.eval(script);
}

fn layout_webviews(window: &Window, chrome: &Webview, content: &Webview, size: PhysicalSize<u32>) {
    let scale_factor = window.scale_factor().unwrap_or(1.0);
    let chrome_height = ((CHROME_HEIGHT * scale_factor).round() as u32).min(size.height);
    let content_height = size.height.saturating_sub(chrome_height);

    let _ = chrome.set_bounds(Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(size.width, chrome_height).into(),
    });
    let _ = content.set_bounds(Rect {
        position: PhysicalPosition::new(0, chrome_height as i32).into(),
        size: PhysicalSize::new(size.width, content_height).into(),
    });
}

fn ensure_trusted_chrome(webview: &Webview) -> Result<(), String> {
    let url = webview
        .url()
        .map_err(|error| format!("无法读取顶栏来源：{error}"))?;
    let is_bundled_page = url.scheme() == "tauri" || is_local_url(url.host_str());
    if webview.label() == CHROME_WEBVIEW_LABEL && is_bundled_page {
        Ok(())
    } else {
        Err("该操作只允许来自本地顶栏".into())
    }
}

#[tauri::command]
fn chrome_snapshot(
    webview: Webview,
    state: tauri::State<'_, ChromeState>,
) -> Result<(String, bool), String> {
    ensure_trusted_chrome(&webview)?;
    let status = state.0.lock().unwrap().as_str().to_string();
    let maximized = webview
        .window()
        .is_maximized()
        .map_err(|error| format!("无法读取窗口状态：{error}"))?;
    Ok((status, maximized))
}

#[tauri::command]
fn chrome_window_action(webview: Webview, action: String) -> Result<bool, String> {
    ensure_trusted_chrome(&webview)?;
    let window = webview.window();
    match action.as_str() {
        "start-dragging" => window
            .start_dragging()
            .map_err(|error| format!("无法拖动窗口：{error}"))?,
        "minimize" => window
            .minimize()
            .map_err(|error| format!("无法最小化窗口：{error}"))?,
        "toggle-maximize" => {
            if window
                .is_maximized()
                .map_err(|error| format!("无法读取窗口状态：{error}"))?
            {
                window
                    .unmaximize()
                    .map_err(|error| format!("无法还原窗口：{error}"))?;
            } else {
                window
                    .maximize()
                    .map_err(|error| format!("无法最大化窗口：{error}"))?;
            }
        }
        "close" => {
            window
                .close()
                .map_err(|error| format!("无法关闭窗口：{error}"))?;
            return Ok(false);
        }
        _ => return Err(format!("未知窗口操作：{action}")),
    }
    Ok(window.is_maximized().unwrap_or(false))
}

#[tauri::command]
fn check_dsh_update(app: AppHandle, webview: Webview) -> Result<(), String> {
    ensure_trusted_chrome(&webview)?;
    let content = app
        .get_webview(CONTENT_WEBVIEW_LABEL)
        .ok_or_else(|| "内容视图尚未创建".to_string())?;
    let update_page_url = app
        .state::<UpdatePageState>()
        .0
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "更新页面尚未准备好".to_string())?;
    let log_dir = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    start_update_check(&app, &content, &log_dir, &update_page_url, true);
    Ok(())
}

/// dsh 更新入口：manual=true（顶栏触发）任何结果都弹窗反馈；false（启动检查）时静默。
/// 确认更新后：主窗口切到本地 update.html（替换已失效的 dsh 界面，更新过程可见、页面不可操作）
/// → 停掉自拉的 dsh → npm 更新 → 单按钮弹窗 → 重启应用收尾。
fn start_update_check(
    app: &AppHandle,
    window: &Webview,
    log_dir: &Path,
    update_page_url: &tauri::Url,
    manual: bool,
) {
    let already_running = {
        let state = app.state::<UpdateState>();
        let mut guard = state.0.lock().unwrap();
        if *guard {
            true
        } else {
            *guard = true;
            false
        }
    };
    if already_running {
        if manual {
            app.dialog()
                .message("已有更新任务在进行中，请稍候。")
                .title("dsh 更新")
                .blocking_show();
        }
        return;
    }

    set_kernel_status(app, KernelStatus::Checking);

    let app_handle = app.clone();
    let window_handle = window.clone();
    let log_dir_own = log_dir.to_path_buf();
    let update_page_url = update_page_url.clone();
    thread::spawn(move || {
        let state = app_handle.state::<UpdateState>();
        let mut final_message: Option<String> = None;
        'flow: {
            let installed = match installed_dsh_version() {
                Ok(version) => version,
                Err(reason) => {
                    if manual {
                        app_handle
                            .dialog()
                            .message(reason)
                            .title("dsh 更新")
                            .kind(MessageDialogKind::Error)
                            .blocking_show();
                    }
                    break 'flow;
                }
            };
            let latest = match latest_dsh_version() {
                Ok(version) => version,
                Err(reason) => {
                    if manual {
                        app_handle
                            .dialog()
                            .message(format!(
                                "无法查询 npm registry，请检查网络或代理后重试。\n\n{reason}"
                            ))
                            .title("dsh 更新")
                            .kind(MessageDialogKind::Error)
                            .blocking_show();
                    }
                    break 'flow;
                }
            };
            if !update_available(&latest, &installed) {
                if manual {
                    app_handle
                        .dialog()
                        .message(format!("当前已是最新版本 v{installed}。"))
                        .title("dsh 更新")
                        .blocking_show();
                }
                break 'flow;
            }
            set_kernel_status(&app_handle, KernelStatus::Available);
            let confirmed = app_handle
                .dialog()
                .message(format!(
                    "发现 dsh 新版本 v{latest}（当前 v{installed}）。\n\n更新期间 dsh 服务将暂时停止，约需 1-2 分钟，请勿关闭应用。"
                ))
                .title("dsh 更新")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "更新".into(),
                    "取消".into(),
                ))
                .blocking_show();
            if !confirmed {
                break 'flow;
            }
            set_kernel_status(&app_handle, KernelStatus::Updating);
            // 主窗口切到本地更新页：更新过程可见，且替换掉已失去后端的 dsh 界面
            if let Err(error) = window_handle.navigate(update_page_url.clone()) {
                app_handle
                    .dialog()
                    .message(format!("无法打开本地更新页面，更新尚未开始：{error}"))
                    .title("dsh 更新")
                    .kind(MessageDialogKind::Error)
                    .blocking_show();
                break 'flow;
            }
            let _ = window_handle.window().set_title("dsh 更新中…");
            // 先停掉自己拉起的 dsh，避免 Windows 文件占用导致 npm 更新失败
            if let Some(mut child) = app_handle.state::<ServerState>().0.lock().unwrap().take() {
                terminate_child(&mut child);
            }
            final_message = Some(match run_dsh_update(&log_dir_own, &latest) {
                Ok(new_version) => format!(
                    "dsh 已更新到 v{new_version}。\n\n点击确定重启应用后生效；你终端里手动运行的 dsh 需要自行重启。"
                ),
                Err(reason) => format!("dsh 更新失败：{reason}\n\n点击确定重启应用以恢复。"),
            });
        }
        *state.0.lock().unwrap() = false;
        if let Some(message) = final_message {
            app_handle
                .dialog()
                .message(message)
                .title("dsh 更新")
                .buttons(MessageDialogButtons::Ok)
                .blocking_show();
            app_handle.restart();
        } else {
            set_kernel_status(&app_handle, KernelStatus::Ready);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例：第二次启动时聚焦已有窗口后退出
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerState::default())
        .manage(UpdateState::default())
        .manage(UpdatePageState::default())
        .manage(ChromeState::default())
        .invoke_handler(tauri::generate_handler![
            chrome_snapshot,
            chrome_window_action,
            check_dsh_update
        ])
        .setup(|app| {
            let port = resolve_port();
            let log_dir = app.path().app_log_dir().unwrap_or_else(|_| PathBuf::from("."));

            // 一个无边框原生窗口承载两个隔离的 WebView：可信本地顶栏 + dsh 内容。
            // 顶栏持久存在；内容视图导航到 localhost 后仍拿不到顶栏的 capability。
            let window = WindowBuilder::new(app, "main")
                .title("DeepSeek Harness")
                // 1280×760 在 125% DPI 的 1080p 工作区内仍完整可见；
                // 原 1280×872 会变成约 1600×1090 物理像素并被任务栏裁切。
                .inner_size(1280.0, 760.0)
                .min_inner_size(900.0, 652.0)
                .decorations(false)
                .theme(Some(Theme::Dark))
                .visible(false)
                .center()
                .build()?;
            let inner_size = window.inner_size()?;
            let chrome_height = ((CHROME_HEIGHT * window.scale_factor()?).round() as u32)
                .min(inner_size.height);

            let chrome = window.add_child(
                WebviewBuilder::new(
                    CHROME_WEBVIEW_LABEL,
                    WebviewUrl::App("chrome.html".into()),
                )
                .on_navigation(|url| {
                    url.scheme() == "tauri" || is_local_url(url.host_str())
                })
                .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny),
                PhysicalPosition::new(0, 0),
                PhysicalSize::new(inner_size.width, chrome_height),
            )?;

            let content = window.add_child(
                WebviewBuilder::new(
                    CONTENT_WEBVIEW_LABEL,
                    WebviewUrl::App("index.html".into()),
                )
                // 内容视图：本地页面/localhost 放行；外链一律转系统浏览器。
                .on_navigation(|url| {
                    if url.scheme() == "tauri" || is_local_url(url.host_str()) {
                        return true;
                    }
                    if matches!(url.scheme(), "http" | "https") {
                        open_external(url.as_str());
                    }
                    false
                })
                .on_new_window(|url, _features| {
                    open_external(url.as_str());
                    tauri::webview::NewWindowResponse::Deny
                }),
                PhysicalPosition::new(0, chrome_height as i32),
                PhysicalSize::new(
                    inner_size.width,
                    inner_size.height.saturating_sub(chrome_height),
                ),
            )?;

            layout_webviews(&window, &chrome, &content, inner_size);
            {
                let event_window = window.clone();
                let event_chrome = chrome.clone();
                let event_content = content.clone();
                window.on_window_event(move |event| match event {
                    WindowEvent::Resized(size) => {
                        layout_webviews(
                            &event_window,
                            &event_chrome,
                            &event_content,
                            *size,
                        );
                        sync_chrome_window_state(&event_window, &event_chrome);
                    }
                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        layout_webviews(
                            &event_window,
                            &event_chrome,
                            &event_content,
                            *new_inner_size,
                        );
                    }
                    WindowEvent::Focused(focused) => {
                        let script = if *focused {
                            "window.setWindowActive?.(true);"
                        } else {
                            "window.setWindowActive?.(false);"
                        };
                        let _ = event_chrome.eval(script);
                    }
                    _ => {}
                });
            }

            // 必须在窗口还停留于本地页面时保存 URL。若从 dsh 的 localhost URL 改 path，
            // 会错误地导航到远程 /update.html，而不是 Tauri 内置资源。
            let mut update_page_url = content.url()?;
            update_page_url.set_path("/update.html");
            update_page_url.set_query(None);
            update_page_url.set_fragment(None);
            app.state::<UpdatePageState>()
                .0
                .lock()
                .unwrap()
                .replace(update_page_url.clone());
            set_kernel_status(app.handle(), KernelStatus::Starting);
            window.show()?;
            window.set_focus()?;

            let app_handle = app.handle().clone();
            thread::spawn(move || {
                let finish_startup = || {
                    let _ = content.navigate(tauri::Url::parse(&url_for(port)).unwrap());
                    set_kernel_status(&app_handle, KernelStatus::Ready);
                    if std::env::var_os("DSH_NO_UPDATE_CHECK").is_none() {
                        start_update_check(
                            &app_handle,
                            &content,
                            &log_dir,
                            &update_page_url,
                            false,
                        );
                    }
                };

                // 1) 已经有 dsh 在服务 → 直接挂上去（退出时不杀它）
                if dsh_serving(port) {
                    finish_startup();
                    return;
                }

                // 2) 端口被别的程序占着 → 给它几秒机会变成 dsh，否则报错
                if tcp_ready(port, Duration::from_secs(1)) {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < deadline {
                        if dsh_serving(port) {
                            finish_startup();
                            return;
                        }
                        thread::sleep(Duration::from_millis(300));
                    }
                    fail(
                        &app_handle,
                        format!(
                            "端口 {port} 已被其它程序占用，且不是 dsh。\n\n可设置环境变量 DSH_WEB_PORT 换一个端口后重启。"
                        ),
                    );
                    return;
                }

                // 3) 端口空闲 → 自己拉起 dsh（先试 PATH 里的 dsh，再试 node 直跑 bin.js）
                let mut child = match spawn_dsh(port, &log_dir) {
                    Ok(child) => child,
                    Err(error) => {
                        fail(&app_handle, format!("无法启动 dsh：{error}"));
                        return;
                    }
                };
                let mut reason = match wait_for_dsh(port, &mut child, START_TIMEOUT) {
                    Ok(()) => {
                        app_handle
                            .state::<ServerState>()
                            .0
                            .lock()
                            .unwrap()
                            .replace(child);
                        finish_startup();
                        return;
                    }
                    Err(reason) => reason,
                };
                terminate_child(&mut child);

                // 兜底：GUI 进程 PATH 里没有 dsh 时，用 node 直跑 npm 全局的 bin.js
                if let Some(fallback) = spawn_dsh_node(port, &log_dir) {
                    match fallback {
                        Ok(mut fallback_child) => {
                            match wait_for_dsh(port, &mut fallback_child, START_TIMEOUT) {
                                Ok(()) => {
                                    app_handle
                                        .state::<ServerState>()
                                        .0
                                        .lock()
                                        .unwrap()
                                        .replace(fallback_child);
                                    finish_startup();
                                    return;
                                }
                                Err(fallback_reason) => {
                                    terminate_child(&mut fallback_child);
                                    reason = fallback_reason;
                                }
                            }
                        }
                        Err(error) => {
                            reason = format!("{reason}；node 直启也失败：{error}");
                        }
                    }
                }
                fail(
                    &app_handle,
                    format!("{reason}\n\n日志：{}\\dsh-server.log", log_dir.display()),
                );
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building dsh-desktop")
        .run(|app_handle, event| {
            // 应用退出时清理自己拉起的 dsh 进程树（挂接的不管）
            if let RunEvent::Exit = event {
                if let Some(mut child) = app_handle.state::<ServerState>().0.lock().unwrap().take() {
                    terminate_child(&mut child);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{dsh_web_args, parse_dsh_version, update_available};

    #[test]
    fn web_launch_disables_external_browser() {
        assert_eq!(dsh_web_args(3080).join(" "), "web --no-open --port 3080");
    }

    #[test]
    fn parses_prefixed_version() {
        assert_eq!(parse_dsh_version("v1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn compares_semver_and_prerelease_versions() {
        let stable = parse_dsh_version("1.0.0").unwrap();
        let rc_two = parse_dsh_version("1.0.0-rc.2").unwrap();
        let rc_ten = parse_dsh_version("1.0.0-rc.10").unwrap();

        assert!(update_available(&stable, &rc_ten));
        assert!(update_available(&rc_ten, &rc_two));
        assert!(!update_available(&rc_two, &stable));
        assert!(!update_available(&stable, &stable));
    }

    #[test]
    fn rejects_invalid_version() {
        assert!(parse_dsh_version("latest").is_err());
    }
}
