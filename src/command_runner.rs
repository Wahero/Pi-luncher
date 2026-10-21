use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// 命令执行结果
#[derive(Debug)]
pub enum RunResult {
    /// 命令成功完成
    Success,
    /// 命令失败，包含退出码和 stderr 摘要
    Failure { code: i32, stderr: String },
    /// 命令未找到（可执行文件不存在）
    NotFound(String),
    /// 启动或等待过程中发生 IO 错误
    IoError(String),
}

/// 服务起動探测结果
#[derive(Debug)]
pub enum StartupOutcome {
    /// 进程存活且已能回应 HTTP
    Up,
    /// 进程在就绪前就退出了
    Exited,
    /// 超时仍未就绪
    Timeout,
}

/// 检查命令是否存在于 PATH 中
pub fn command_exists(name: &str) -> bool {
    let check_cmd = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };

    Command::new(check_cmd)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 解析要启动的程序。
///
/// Windows 上 npm 全局命令（如 `pi`、`pi-web`）实际是 `.cmd` / `.bat` shim，
/// `CreateProcess` 无法直接执行它们（会得到 NotFound），必须经 `cmd.exe` 包装。
/// 返回 `(真实路径, 是否需要经 cmd 包装)`：
/// - 找到 `.exe` → 直接启动
/// - 找到 `.cmd` / `.bat` → 用 `cmd /C` 启动
/// - 找不到 → 原样交给 `cmd /C` 解析
#[cfg(windows)]
fn resolve_launch(program: &str) -> (String, bool) {
    if let Ok(output) = Command::new("where").arg(program).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut exe = None;
            let mut script = None;
            for line in text.lines() {
                let p = line.trim();
                if p.is_empty() {
                    continue;
                }
                let lower = p.to_ascii_lowercase();
                if lower.ends_with(".exe") && exe.is_none() {
                    exe = Some(p.to_string());
                } else if (lower.ends_with(".cmd") || lower.ends_with(".bat")) && script.is_none() {
                    script = Some(p.to_string());
                }
            }
            if let Some(e) = exe {
                return (e, false);
            }
            if let Some(s) = script {
                return (s, true);
            }
        }
    }
    (program.to_string(), true)
}

#[cfg(not(windows))]
fn resolve_launch(program: &str) -> (String, bool) {
    (program.to_string(), false)
}

/// 构造一个 `Command`，统一处理跨平台（尤其是 Windows `.cmd` shim）的启动差异。
pub fn build_command(program: &str) -> Command {
    let (real, via_cmd) = resolve_launch(program);
    if via_cmd {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(real);
        cmd
    } else {
        Command::new(real)
    }
}

/// 命令的原始输出（stdout / stderr 与退出码），供需要读取输出内容的场景使用。
#[derive(Debug)]
pub struct RawOutput {
    pub success: bool,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// 运行一个前台命令并捕获其输出，本身不判定成败；
/// 仅当命令无法启动（未找到 / IO 错误）时返回 Err。
pub fn run_capture(program: &str, args: &[&str]) -> Result<RawOutput, RunResult> {
    let output = match build_command(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(RunResult::NotFound(program.to_string()));
        }
        Err(e) => return Err(RunResult::IoError(e.to_string())),
    };

    Ok(RawOutput {
        success: output.status.success(),
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        stderr: String::from_utf8_lossy(&output.stderr)
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// 运行一个前台命令，等待其完成，并检查退出状态。
/// stdout/stderr 会被捕获，失败时提取摘要。
pub fn run_checked(program: &str, args: &[&str]) -> RunResult {
    match run_capture(program, args) {
        Ok(o) if o.success => RunResult::Success,
        Ok(o) => RunResult::Failure {
            code: o.code,
            stderr: o.stderr,
        },
        Err(e) => e,
    }
}

/// 以后台方式启动一个常驻服务，并使其脱离本进程 / 控制台。
///
/// 关键点：父进程（launcher）退出、或终端视窗被关闭时，子进程不会被连带终止。
/// - Windows：`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，不附着任何 console，
///   因此 Ctrl+C / 关窗都不会波及它。
/// - Unix：`process_group(0)` 另开进程组。
pub fn spawn_detached(program: &str, args: &[&str]) -> Result<Child, String> {
    let mut cmd = build_command(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn()
        .map_err(|e| format!("无法启动 {}: {}", program, e))
}

/// 对 `host:port` 做一次最小 HTTP 探测，确认服务真的在回应（而不只是端口开着）。
pub fn probe_http(host: &str, port: u16) -> bool {
    let addr = match (host, port).to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => return false,
        },
        Err(_) => return false,
    };

    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

    let request = format!(
        "GET / HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 16];
    match stream.read(&mut buf) {
        Ok(n) if n >= 5 => buf.starts_with(b"HTTP/"),
        _ => false,
    }
}

/// 端口上到底跑的是什么
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebProbe {
    /// 端口没人应答，或应答不是 HTTP
    Down,
    /// 是 pi-web：HTTP 就绪，且响应带 pi-web 自身标识
    Up,
    /// 端口上有别的 HTTP 服务（能应答，但不像 pi-web）
    Foreign,
}

/// 身份探测：任何本地 Web 服务都能让 `probe_http` 的「有 HTTP 回应」通过，
/// 只有这里能区分「pi-web 已在运行」和「端口被别的服务占用」。
///
/// 标识取 pi-web 首页的 `<title>Pi Web</title>`（版本间稳定），
/// 另兼容 `X-Powered-By: pi-web` 之类的显式声明。
pub fn probe_pi_web(host: &str, port: u16) -> WebProbe {
    let text = match fetch_response_head(host, port, 16 * 1024) {
        Some(t) if t.starts_with("HTTP/") => t,
        _ => return WebProbe::Down,
    };
    if looks_like_pi_web(&text) {
        WebProbe::Up
    } else {
        WebProbe::Foreign
    }
}

/// 读取 HTTP 响应开头（状态行 + 头 + 页面开头），最多 `limit` 字节。
/// 只用于身份判定，失败一律当作「没有回应」。
fn fetch_response_head(host: &str, port: u16, limit: usize) -> Option<String> {
    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

    let request = format!(
        "GET / HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 2048];
    while buf.len() < limit {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let take = n.min(limit - buf.len());
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    if buf.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

/// 响应是否来自 pi-web（大小写与标签内空白不敏感）
fn looks_like_pi_web(response: &str) -> bool {
    let lower = response.to_ascii_lowercase();
    let squeezed: String = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.contains("<title>pi web</title>")
        || lower.contains("x-powered-by: pi-web")
        || lower.contains("x-powered-by:pi-web")
}

/// 等待服务真正就绪：既确认进程没有退出，也确认 HTTP 端口能回应。
/// progress：可选回调，按等待进度触发（0..1），供确定进度条爬升使用。
pub fn wait_ready<P: FnMut(f64)>(
    child: &mut Child,
    host: &str,
    port: u16,
    timeout_secs: u64,
    mut progress: Option<P>,
) -> StartupOutcome {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let started = Instant::now();
    loop {
        if let Ok(Some(_status)) = child.try_wait() {
            return StartupOutcome::Exited;
        }
        if probe_http(host, port) {
            if let Some(cb) = progress.as_mut() {
                cb(1.0);
            }
            return StartupOutcome::Up;
        }
        if let Some(cb) = progress.as_mut() {
            let frac = (started.elapsed().as_secs_f64() / timeout_secs as f64).min(1.0);
            cb(frac);
        }
        if Instant::now() >= deadline {
            return StartupOutcome::Timeout;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// 按 PID 终止进程及其整棵进程树。///
/// 后台 pi-web 经 `cmd /C` 启动，真正的服务是其子进程（node），
/// 只 kill 父进程会留下孤儿进程占用端口，因此需要连同子进程一起终止。
pub fn terminate_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// 终止进程树并回收句柄（用于启动失败 / 超时的半途进程）。
pub fn terminate_background(child: &mut Child) {
    terminate_pid(child.id());
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// 起一个只应答一次的假 HTTP 服务，返回端口
    fn serve_once(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定随机端口");
        let port = listener.local_addr().expect("读取端口").port();
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let resp = format!("HTTP/1.0 200 OK\r\nContent-Type: text/html\r\n\r\n{}", body);
                let _ = s.write_all(resp.as_bytes());
            }
        });
        port
    }

    #[test]
    fn probe_recognizes_pi_web() {
        let port = serve_once("<html><head><title>Pi Web</title></head></html>");
        assert_eq!(probe_pi_web("127.0.0.1", port), WebProbe::Up);
    }

    #[test]
    fn probe_rejects_other_http_service() {
        let port = serve_once("<html><head><title>Some Other App</title></head></html>");
        assert_eq!(probe_pi_web("127.0.0.1", port), WebProbe::Foreign);
    }

    #[test]
    fn probe_reports_down_when_nothing_listens() {
        // 绑定后立即释放，得到一个当前空闲的端口
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").expect("绑定随机端口");
            l.local_addr().expect("读取端口").port()
        };
        assert_eq!(probe_pi_web("127.0.0.1", port), WebProbe::Down);
    }

    #[test]
    fn marker_matching_is_case_insensitive() {
        assert!(looks_like_pi_web("<title>Pi Web</title>"));
        assert!(looks_like_pi_web("<TITLE>PI WEB</TITLE>"));
        assert!(looks_like_pi_web("X-Powered-By: pi-web"));
        assert!(!looks_like_pi_web("<title>Grafana</title>"));
        assert!(!looks_like_pi_web("HTTP/1.0 404 Not Found"));
    }
}
