mod command_runner;
mod steps;
mod ui;

use command_runner::spawn_detached;
use std::process::{self, Child};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use steps::{
    step_pi_cli, step_pi_update, step_pi_web, step_pi_web_upgrade, step_preflight, WebStart,
};
use ui::{Display, Mode, Ui};
use unicode_width::UnicodeWidthStr;

const DEFAULT_PORT: u16 = 30141;
const DEFAULT_HOST: &str = "127.0.0.1";
const MAX_RESTARTS: u32 = 3;

/// 正常模式结束停留：30 秒无输入自动关闭窗口
const WAIT_AUTOCLOSE_SECS: u64 = 30;

const BANNER_TITLE_TEXT: &str = "Pi Agent 批量自动化启动工具";

struct Args {
    mode: Mode,
    display: Display,
    spin: Option<String>,
    force_tty: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        mode: Mode::Normal,
        display: Display::Ledger,
        spin: None,
        force_tty: false,
    };

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--quiet" | "-q" | "--auto" => args.mode = Mode::Quiet,
            "--force-tty" => args.force_tty = true, // 验证用法：强制走动画渲染路径
            "--spin" => {
                if let Some(name) = it.next() {
                    args.spin = Some(name);
                }
            }
            "--display" => {
                let val = it.next().unwrap_or_default();
                match Display::from_arg(&val) {
                    Some(d) => args.display = d,
                    None => {
                        println!(
                            "  未知显示模式 '{}'（--display 可选 A|B|C，即 ledger|lite|min）",
                            val
                        );
                        process::exit(1);
                    }
                }
            }
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            other => {
                println!("  未知参数 '{}'（见 --help）", other);
                process::exit(1);
            }
        }
    }
    args
}

fn print_help() {
    println!();
    println!("  pi-launcher v{}", ui::VERSION);
    println!("  ──────────────────────────────────────────────────────────────");
    println!("  Pi Agent 启动器：环境预检 → pi update → 检查并升级 pi-web →");
    println!("                  启动 pi-web（后台常驻）→ 启动 Pi CLI");
    println!();
    println!("  用法：pi-launcher [--quiet] [--display <A|B|C>] [--spin <名字>] [--force-tty]");
    println!("    （无参数）             正常模式：输出 + 动画 + pi CLI + 结束停留（30 秒自动关）");
    println!("    --quiet / -q / --auto  安静模式：零输出、跳过 pi CLI、失败/完成都自动关窗");
    println!(
        "    --display <A|B|C>      显示模式：A=工作账·一步一行（默认）B=轻修·分栏仪表 C=极简·一行流"
    );
    println!(
        "                           （别名均可：a/b/c、ledger/lite/min；quiet 与非终端下无效）"
    );
    println!(
        "    --spin <名字>          指定右侧动画（循环池：{}）",
        ui::anim_pool()
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join("/")
    );
    println!("    --force-tty            验证用法：非终端环境也强制走动画渲染路径");
    println!();
    println!(
        "  日志（两种模式均写）：{}（保留最近 {} 份）",
        ui::log_dir().to_string_lossy(),
        ui::LOG_KEEP
    );
    println!();
    println!("  退出码：0=全部成功 1=预检失败 2=pi update 失败 3=pi-web 升级失败");
    println!("          4=pi-web 启动失败 5=Pi CLI 异常");
    println!();
}

fn env_host_port() -> (String, u16) {
    let host = std::env::var("PI_WEB_HOSTNAME").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    (host, port)
}

fn main() {
    let args = parse_args();

    // 日志先行（两种模式都要有），banner 等输出随后才有落点
    if let Err(e) = ui::init_logger() {
        println!("  ⚠ 日志文件创建失败：{}", e);
    }

    // Ctrl+C：pi-web 已 detach，无需清理，仅提示后退出。
    let _ = ctrlc::set_handler(|| {
        println!();
        println!("  收到中断信号，launcher 退出；pi-web 仍在后台运行（停止：pi-web stop）。");
        process::exit(130);
    });

    let (host, port) = env_host_port();

    let code = match args.mode {
        Mode::Quiet => run_quiet(host, port),
        Mode::Normal => run_normal(&args, host, port),
    };
    process::exit(code);
}

// ────────────────────────────────────────────────────────────────
// 步骤 1-4 的流程（两模式共用；返回 WebStart 与退出码）
// ────────────────────────────────────────────────────────────────

fn boot_pipeline(ui: &mut Ui, host: &str, port: u16) -> Result<WebStart, i32> {
    if step_preflight(ui).is_err() {
        return Err(1);
    }
    if step_pi_update(ui).is_err() {
        return Err(2);
    }
    if step_pi_web_upgrade(ui, host, port).is_err() {
        return Err(3);
    }
    match step_pi_web(ui, host, port) {
        Ok(ws) => Ok(ws),
        Err(_e) => Err(4),
    }
}

// ────────────────────────────────────────────────────────────────
// 安静模式：零 UI、跳过 pi CLI、失败也自动关窗
// ────────────────────────────────────────────────────────────────

fn run_quiet(host: String, port: u16) -> i32 {
    let mut ui = Ui::init(
        Mode::Quiet,
        Display::Ledger, // quiet 零输出，显示模式不参与渲染
        false,
        None,
        steps::TOTAL_STEPS,
    );
    ui::log("启动模式：安静（--quiet；零输出、跳过 pi CLI、自动关窗）");

    println!("  pi-launcher（--quiet）：静默启动中，完成后窗口自动关闭。");
    println!("    日志：{}", ui::log_path());

    match boot_pipeline(&mut ui, &host, port) {
        Ok(ws) => {
            if let WebStart::Spawned(child) = ws {
                spawn_supervisor(child, host, port);
            }
            ui::log("pi CLI 已按安静模式策略跳过。");
            ui::log("全部步骤完成；退出码 0（pi-web 若已启动，独立后台运行）");
            0
        }
        Err(code) => {
            ui::log(&format!("最终退出码：{}", code));
            code
        }
    }
}

// ────────────────────────────────────────────────────────────────
// 正常模式：banner + 动画渲染 + pi CLI + 结束停留（30 秒自动关）
// ────────────────────────────────────────────────────────────────

fn run_normal(args: &Args, host: String, port: u16) -> i32 {
    let mut ui = Ui::init(
        Mode::Normal,
        args.display,
        args.force_tty,
        args.spin.as_deref(),
        steps::TOTAL_STEPS,
    );
    ui::log("启动模式：正常（pi CLI 交互 + 结束停留）");
    print_banner(&ui, args.display);

    match boot_pipeline(&mut ui, &host, port) {
        Err(code) => {
            // 失败：撤场并把凝固行（含 ✗ 行）重放为普通行，收尾倒数走纯 stdout
            ui.replay_ledger();
            wait_and_exit(code);
        }
        Ok(WebStart::Spawned(child)) => {
            spawn_supervisor(child, host.clone(), port);
        }
        Ok(WebStart::AlreadyRunning) => {}
    }

    if step_pi_cli(&mut ui).is_err() {
        wait_and_exit(5);
    }

    // 成功收尾：撤掉活动渲染并把账目行原样重放，让收尾信息排在账目之后
    ui.replay_ledger();
    ui.message(&format!("  📝 本次启动日志：{}", ui::log_path()));
    ui.message("  ✓ 全部步骤完成。");
    wait_and_exit(0)
}

/// 开场画面：按显示模式分三档绘制（对齐一律走显示宽度，防中英混排错位）
fn print_banner(ui: &Ui, display: Display) {
    let anim_text: Option<String> = ui
        .anim_label()
        .map(|(i, name)| (i, name, ui::anim_pool().len()))
        .map(|(i, name, n)| format!("动画 {} · 池 {}/{} 组", name, i + 1, n));

    match display {
        Display::Min => {
            ui.message("");
            let head = match &anim_text {
                Some(a) => format!(
                    "  {} {} · {}",
                    console::style("pi-launcher").bold(),
                    console::style(&format!("v{}", ui::VERSION)).dim(),
                    a
                ),
                None => format!(
                    "  {} {}",
                    console::style("pi-launcher").bold(),
                    console::style(&format!("v{}", ui::VERSION)).dim()
                ),
            };
            ui.message(&head);
            ui.message("");
        }
        Display::Lite => {
            ui.message("");
            let title = format!("{} · v{}", BANNER_TITLE_TEXT, ui::VERSION);
            let inner = 46usize;
            let body = ui::pad_width(&format!(" {}", title), inner);
            ui.message(&format!("  ╭{}╮", "─".repeat(inner)));
            ui.message(&format!("  │{}│", body));
            ui.message(&format!("  ╰{}╯", "─".repeat(inner)));
            if let Some(a) = &anim_text {
                ui.message(&format!("  {}（--spin <名字> 可指定）", console::style(a).dim()));
            }
            ui.message("");
        }
        Display::Ledger => {
            ui.message("");
            let left = format!("PI LAUNCHER · v{}", ui::VERSION);
            let cols = 78usize;
            if let Some(a) = &anim_text {
                let gap = cols
                    .saturating_sub(UnicodeWidthStr::width(left.as_str()))
                    .saturating_sub(UnicodeWidthStr::width(a.as_str()))
                    .max(2);
                ui.message(&format!(
                    "  {}{}{}",
                    console::style(&left).bold(),
                    " ".repeat(gap),
                    console::style(a).dim()
                ));
            } else {
                ui.message(&format!("  {}", console::style(&left).bold()));
            }
            ui.message(&format!("  {}", console::style("─".repeat(cols)).dim()));
        }
    }
}

// ────────────────────────────────────────────────────────────────
// 结束停留：按 Enter 立即关闭；30 秒无输入自动关闭（含失败停留路径）
// ────────────────────────────────────────────────────────────────

fn wait_and_exit(code: i32) -> ! {
    pause_before_exit(code);
    process::exit(code);
}

fn pause_before_exit(exit_code: i32) {
    let term = console::Term::stdout();
    if !term.is_term() {
        ui::log(&format!("非交互终端，直接关闭；退出码 {}", exit_code));
        println!("\n  （非交互终端，直接关闭）");
        return;
    }

    // 清空 pi CLI/子进程留下的输入残键：如果停留在开始时输入队列里已经挂着
    // 一个未消费的换行/按键，下面的 read_line 会立即读到 → 「按 Enter 立即关窗」
    // 被瞬间触发，30 秒停留完全没跑起来（v1.1.2 修复）。清场之后重新计数，
    // 只有本次停留期间的新输入才算「按 Enter 立即关闭」。
    flush_pending_console_input();

    println!();
    let pressed = Arc::new(AtomicBool::new(false));
    {
        let pressed = Arc::clone(&pressed);
        let secs = WAIT_AUTOCLOSE_SECS;
        let code = exit_code;
        thread::spawn(move || {
            for remain in (1..=secs).rev() {
                if pressed.load(Ordering::SeqCst) {
                    return;
                }
                print!("\r  按 Enter 键关闭此窗口 · {} 秒后自动关闭 ...   ", remain);
                let _ = std::io::Write::flush(&mut std::io::stdout());
                thread::sleep(Duration::from_secs(1));
            }
            if !pressed.load(Ordering::SeqCst) {
                ui::log("30 秒无输入，自动关闭");
                // 倒数超时沿用本次退出码（修复：此前一律 exit(0)，
                // 失败停留的超时关窗会被计划任务误判为成功）
                process::exit(code);
            }
        });
    }

    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    pressed.store(true, Ordering::SeqCst);
    println!();
}

/// 清空本进程控制台的等待输入队列（仅 Windows 实现；其它平台无对应队列）。
#[cfg(windows)]
fn flush_pending_console_input() {
    use windows_sys::Win32::System::Console::{
        FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE,
    };
    unsafe {
        let _ = FlushConsoleInputBuffer(GetStdHandle(STD_INPUT_HANDLE));
    }
}

#[cfg(not(windows))]
fn flush_pending_console_input() {}

// ────────────────────────────────────────────────────────────────
// pi-web 守护线程：崩溃自动重启（最多 MAX_RESTARTS 次）+ HTTP 健康告警
// ────────────────────────────────────────────────────────────────

fn spawn_supervisor(mut child: Child, host: String, port: u16) {
    thread::spawn(move || {
        let mut restarts = 0u32;
        let mut unhealthy = 0u32;
        let mut ticks = 0u64;

        loop {
            thread::sleep(Duration::from_millis(2000));
            ticks += 1;

            match child.try_wait() {
                Ok(Some(status)) => {
                    ui::notify(&format!(
                        "  ⚠ [supervisor] 侦测到 pi-web 已退出（{}）",
                        status
                    ));
                    if restarts >= MAX_RESTARTS {
                        ui::notify(&format!(
                            "  ✗ [supervisor] 已重启 {} 次仍未稳定，停止守护；请手动执行 `pi-web` 排查。",
                            restarts
                        ));
                        return;
                    }
                    restarts += 1;
                    ui::notify(&format!(
                        "  ↻ [supervisor] 正在重启 pi-web（第 {} 次）…",
                        restarts
                    ));

                    match spawn_detached("pi-web", &[]) {
                        Ok(mut c) => {
                            match command_runner::wait_ready(
                                &mut c,
                                &host,
                                port,
                                30,
                                Some(|_: f64| {}),
                            ) {
                                command_runner::StartupOutcome::Up => {
                                    ui::notify("  ✓ [supervisor] pi-web 已重启并就绪");
                                    child = c;
                                    unhealthy = 0;
                                }
                                other => {
                                    ui::notify(&format!(
                                        "  ⚠ [supervisor] pi-web 重启后未就绪：{:?}",
                                        other
                                    ));
                                    child = c;
                                }
                            }
                        }
                        Err(e) => {
                            ui::notify(&format!("  ✗ [supervisor] 无法重启 pi-web：{}", e));
                            return;
                        }
                    }
                }
                Ok(None) => {
                    // 每 ~10 秒做一次 HTTP 健康探测
                    if ticks.is_multiple_of(5) {
                        if command_runner::probe_http(&host, port) {
                            unhealthy = 0;
                        } else {
                            unhealthy += 1;
                            if unhealthy >= 3 {
                                ui::notify(&format!(
                                    "  ⚠ [supervisor] pi-web 进程存活但 HTTP 无响应（连续 {} 次）",
                                    unhealthy
                                ));
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
    });
}
