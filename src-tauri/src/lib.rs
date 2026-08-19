use std::{
    fs::File,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// dsh web 端默认端口（dsh-web-app 的 cordis.patch.yml：`port: !!js ctx.webStartup.port ?? 3080`）
const DEFAULT_PORT: u16 = 3080;
/// 等待 dsh 服务就绪的最长时间
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// dsh 前端 <title> 标记，用于区分“3080 上真的是 dsh”与“被别的程序占了”
const DSH_MARKER: &str = "DeepSeek Harness";

/// 由本客户端拉起的 dsh 子进程（挂接到已存在服务时保持 None，退出时不杀它）
#[derive(Default)]
struct ServerState(Mutex<Option<Child>>);

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
fn spawn_with(_port: u16, log_dir: &PathBuf, program: &str, args: &[String]) -> std::io::Result<Child> {
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

/// 主路径：`cmd /C dsh web --port <port>`（依赖 PATH 里的 dsh）
fn spawn_dsh(port: u16, log_dir: &PathBuf) -> std::io::Result<Child> {
    spawn_with(
        port,
        log_dir,
        "cmd",
        &["/C".into(), format!("dsh web --port {port}")],
    )
}

/// 兜底：直接用 node 跑 npm 全局安装的 dsh bin.js（防 GUI 进程 PATH 里没有 dsh.cmd）
fn spawn_dsh_node(port: u16, log_dir: &PathBuf) -> Option<std::io::Result<Child>> {
    let appdata = std::env::var("APPDATA").ok()?;
    let bin = PathBuf::from(appdata).join(r"npm\node_modules\@deepseek-ai\dsh\lib\bin.js");
    if !bin.exists() {
        return None;
    }
    Some(spawn_with(
        port,
        log_dir,
        "node",
        &[
            bin.display().to_string(),
            "web".into(),
            "--port".into(),
            port.to_string(),
        ],
    ))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例：第二次启动时聚焦已有窗口后退出
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerState::default())
        .setup(|app| {
            let port = resolve_port();
            let log_dir = app.path().app_log_dir().unwrap_or_else(|_| PathBuf::from("."));

            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("DeepSeek Harness")
                    .inner_size(1280.0, 820.0)
                    .min_inner_size(900.0, 600.0)
                    .center()
                    // 窗口内导航：本地页面/localhost 放行；外链一律转系统浏览器
                    .on_navigation(|url| {
                        if url.scheme() == "tauri" || is_local_url(url.host_str()) {
                            return true;
                        }
                        if matches!(url.scheme(), "http" | "https") {
                            open_external(url.as_str());
                        }
                        false
                    })
                    // window.open / target=_blank：不开新 webview，转系统浏览器
                    .on_new_window(|url, _features| {
                        open_external(url.as_str());
                        tauri::webview::NewWindowResponse::Deny
                    })
                    .build()?;

            let app_handle = app.handle().clone();
            thread::spawn(move || {
                // 1) 已经有 dsh 在服务 → 直接挂上去（退出时不杀它）
                if dsh_serving(port) {
                    let _ = window.navigate(tauri::Url::parse(&url_for(port)).unwrap());
                    return;
                }

                // 2) 端口被别的程序占着 → 给它几秒机会变成 dsh，否则报错
                if tcp_ready(port, Duration::from_secs(1)) {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < deadline {
                        if dsh_serving(port) {
                            let _ = window.navigate(tauri::Url::parse(&url_for(port)).unwrap());
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
                        let _ = window.navigate(tauri::Url::parse(&url_for(port)).unwrap());
                        return;
                    }
                    Err(reason) => reason,
                };
                let _ = child.kill();

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
                                    let _ =
                                        window.navigate(tauri::Url::parse(&url_for(port)).unwrap());
                                    return;
                                }
                                Err(fallback_reason) => {
                                    let _ = fallback_child.kill();
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
                if let Some(child) = app_handle.state::<ServerState>().0.lock().unwrap().take() {
                    kill_tree(child.id());
                }
            }
        });
}
