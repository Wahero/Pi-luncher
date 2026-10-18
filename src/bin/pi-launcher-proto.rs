////////////////////////////////////////////////////////////////////////////////
// pi-launcher-proto —— 可行性审查用原型（验收通过后可整体删除）
//
// 目的：在实现真实功能之前，先完整演示重构后的 UI 形态：
//   * 每个主要步骤「左消息 + 右侧固定位置进度条」
//   * 底部常驻「整体进度条」（0-100%，按 5 步权重推进）
//   * pi-web 等待就绪 = 真百分比（按秒推进）；npm 类无真实进度的任务 = 脉冲条（不造假百分比）
//   * --quiet / --auto 安静模式：无 UI、跳过 pi CLI、失败也自动关窗
//   * 两种模式都写文字日志（每次启动新文件，自动保留最近 20 份）——这部分是真实实现
//
// 「工作本身」全部为模拟（sleep），不执行 pi / npm / pi-web。
//
// 用法：
//   pi-launcher-proto                    # 正常模式（模拟全部 5 步）
//   pi-launcher-proto --quiet            # 安静模式（自动关窗，只留日志）
//   pi-launcher-proto --fail             # 正常模式：步骤 4 模拟失败（退出码 4）
//   pi-launcher-proto --quiet --fail     # 安静模式：步骤 4 模拟失败（退出码 4）
////////////////////////////////////////////////////////////////////////////////

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

const VERSION: &str = "0.1.0-proto";

/// pi-web 就绪等待上限——原型缩为 5s 方便演示（正式版为 60s）
const READY_SIM_SECS: f64 = 5.0;

/// 动画池：每次启动循环（rotate）选用一个；--spin <名字> 可指定
/// （用户选定的集合：dots / dots2 / growVertical / moon / clock / hourglass）
fn spinners() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dots", "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        ("dots2", "⣾⣽⣻⢿⡿⣟⣯⣷"),
        ("grow", "▁▃▄▅▆▇▆▅▄▃"),
        ("moon", "🌑🌒🌓🌔🌕🌖🌗🌘"),
        ("clock", "🕐🕑🕒🕓🕔🕕🕖🕗🕘🕙🕚🕛"),
        ("hourglass", "⏳⌛"),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Quiet,
}

struct Task {
    label: &'static str,
    /// 模拟耗时（秒）
    secs: f64,
    /// false = 脉冲条（npm 类无真实进度）；true = 确定进度条（等待就绪类）
    deterministic: bool,
}

fn tasks() -> Vec<Task> {
    vec![
        Task {
            label: "环境预检（pi / pi-web / npm）",
            secs: 0.8,
            deterministic: false,
        },
        Task {
            label: "执行 pi update --all",
            secs: 4.0,
            deterministic: false,
        },
        Task {
            label: "检查并升级 pi-web",
            secs: 3.0,
            deterministic: false,
        },
        Task {
            label: "启动 pi-web（等待 HTTP 就绪 127.0.0.1:30141，原型缩短为 5s）",
            secs: READY_SIM_SECS,
            deterministic: true,
        },
        Task {
            label: "启动 pi CLI 交互界面（安静模式将跳过；此处模拟退出）",
            secs: 1.6,
            deterministic: false,
        },
    ]
}

/// 步骤中途切换的子消息（时间秒阈值, 消息）
fn sub_messages(idx: usize) -> Vec<(f64, &'static str)> {
    match idx {
        2 => vec![
            (0.0, "查询 registry 最新版本…（模拟）"),
            (1.2, "npm install -g @agegr/pi-web@latest …（模拟）"),
        ],
        4 => vec![(0.0, "pi CLI 运行中…"), (1.2, "pi CLI 正常退出（模拟）")],
        _ => vec![],
    }
}

fn success_msg(idx: usize) -> String {
    match idx {
        0 => "pi / pi-web / npm 均已就绪".to_string(),
        1 => "Pi 及所有扩展包更新完成".to_string(),
        2 => "pi-web 已是最新版本（v1.2.3，模拟数据）".to_string(),
        3 => "pi-web 已就绪：http://127.0.0.1:30141".to_string(),
        _ => "Pi CLI 已正常退出".to_string(),
    }
}

fn main() {
    let mut mode = Mode::Normal;
    let mut fail = false;
    let mut force_tty = false;
    let mut spin_forced: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--quiet" | "-q" | "--auto" => mode = Mode::Quiet,
            "--fail" => fail = true,
            "--force-tty" => force_tty = true, // 验证用途：强制走动画渲染路径
            "--spin" => {
                if let Some(name) = args.next() {
                    spin_forced = Some(name);
                }
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                println!("  未知参数 '{}'（见 --help）", other);
                process::exit(1);
            }
        }
    }

    let spin = pick_spin(spin_forced.as_deref());

    match mode {
        Mode::Quiet => quiet_flow(fail),
        Mode::Normal => {
            // 非 TTY（管道/CI）：无 bar，退化为纯文字输出（避免 indicatif 全部隐藏导致无输出）
            if force_tty || console::Term::stdout().is_term() {
                normal_flow(fail, spin)
            } else {
                plain_flow(fail, spin)
            }
        }
    }
}

/// 从动画池选用一个：--spin 指定 > 否则按「本机已有日志数」循环轮换
/// （每次启动都会产生一个新日志文件，日志数即启动次数，天然形成循环；
///   正式版沿用同一机制，也可改为随机轮换。）
fn pick_spin(forced: Option<&str>) -> (usize, &'static str, &'static str) {
    let list = spinners();

    if let Some(name) = forced {
        if let Some(i) = list.iter().position(|(n, _)| *n == name) {
            return (i, list[i].0, list[i].1);
        }
        println!(
            "  ⚠ 未知动画名 '{}'，改用循环轮换（可选：{}）",
            name,
            list.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        );
    }

    let runs = existing_run_count();
    let i = runs % list.len();
    (i, list[i].0, list[i].1)
}

fn existing_run_count() -> usize {
    let dir = Logger::dir();
    fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.starts_with("pi-launcher-proto-") && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn print_help() {
    println!();
    println!("  pi-launcher-proto {}", VERSION);
    println!("  ────────────────────────────────────────────────────");
    println!("  可行性审查原型：全部工作为模拟，仅演示 UI / 模式 / 日志形态。");
    println!();
    println!("  用法：pi-launcher-proto [--quiet] [--fail]");
    println!(
        "    （无参数）             正常模式：进度条 + 模拟 5 步 + 结束停留（30 秒后自动关闭）"
    );
    println!("    --quiet / -q / --auto  安静模式：无 UI、跳过 pi CLI、自动关窗");
    println!("    --fail                 步骤 4 模拟失败（退出码 4）");
    println!("    --force-tty            验证用法：非终端环境也强制走动画渲染路径");
    println!(
        "    --spin <名字>          指定动画（循环池 {} 组：{}）",
        spinners().len(),
        spinners()
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join("/")
    );
    println!();
    println!("  日志（真实写入）：两种模式均记录到 %LOCALAPPDATA%\\pi-launcher-proto\\logs\\，保留最近 20 份。");
    println!();
}

fn banner() {
    // 与正式版 main.rs 相同的对齐策略：按 unicode 显示宽度计算空格，
    // 中英混排（CJK 双宽）也能保证右边框严格对齐。
    let title = format!("Pi Agent 批量自动化启动工具 {}", VERSION);
    let inner = 42usize;
    let pad = inner.saturating_sub(UnicodeWidthStr::width(title.as_str()));
    let left = pad / 2;
    let right = pad - left;

    println!();
    println!("  ╔{}╗", "═".repeat(inner));
    println!("  ║{}{}{}║", " ".repeat(left), title, " ".repeat(right));
    println!("  ╚{}╝", "═".repeat(inner));
}

/// 结束前停留：按 Enter 立即关闭；30 秒无输入自动关闭（含失败停留路径）。
/// 安静模式不经过这里（始终自动关窗）；非交互终端直接关闭，避免测试卡死。
fn pause_before_exit() {
    let term = console::Term::stdout();
    if !term.is_term() {
        println!("\n  （非交互终端，直接关闭）");
        return;
    }

    const WAIT_SECS: u64 = 30;
    println!();

    let pressed = Arc::new(AtomicBool::new(false));
    {
        let pressed = Arc::clone(&pressed);
        thread::spawn(move || {
            for remain in (1..=WAIT_SECS).rev() {
                if pressed.load(Ordering::SeqCst) {
                    return;
                }
                print!("\r  按 Enter 键关闭此窗口 · {} 秒后自动关闭 ...   ", remain);
                let _ = io::stdout().flush();
                thread::sleep(Duration::from_secs(1));
            }
            if !pressed.load(Ordering::SeqCst) {
                println!("\r  30 秒无输入，已自动关闭。                      ");
                process::exit(0);
            }
        });
    }

    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    pressed.store(true, Ordering::SeqCst);
    println!();
}

/// 非 TTY 的正常模式退化：纯文字（同样写日志），进度改为文本百分比
fn plain_flow(fail: bool, spin: (usize, &'static str, &'static str)) {
    let tasks = tasks();
    let total_secs: f64 = tasks.iter().map(|t| t.secs).sum();
    let steps_total = tasks.len();

    println!("  本次动画：{}（纯文字模式仅标注名称，无动画渲染）", spin.1);

    let mut log = match Logger::new() {
        Ok(l) => l,
        Err(e) => {
            println!("  无法创建日志文件：{}", e);
            process::exit(1);
        }
    };
    log.w("启动模式：正常（非 TTY 纯文字退化，原型模拟）");
    let log_path = log.path.to_string_lossy().to_string();

    let mut done = 0f64;
    for (i, task) in tasks.iter().enumerate() {
        println!("\n  [{}/{}] {}", i + 1, steps_total, task.label);
        log.w(&format!(
            "[STEP {}/{}] 开始：{}",
            i + 1,
            steps_total,
            task.label
        ));
        thread::sleep(Duration::from_secs_f64(task.secs));
        done += task.secs;

        if i == 3 && fail {
            let msg = "pi-web 启动失败（模拟：端口被占用或配置有误）";
            println!("  ✗ {}", msg);
            log.w(&format!("[STEP 4] 失败：{}", msg));
            log.w("最终退出码：4");
            pause_before_exit();
            process::exit(4);
        }

        let msg = success_msg(i);
        println!("  ✓ {}", msg);
        println!("  整体进度 {:>3}%", ((done / total_secs) * 100.0) as u64);
        log.w(&format!("[STEP {}/{}] 完成：{}", i + 1, steps_total, msg));
    }

    println!("\n  📝 本次启动日志：{}", log_path);
    println!("  ✓ 全部步骤完成（退出码 0，原型）");
    log.w("全部步骤完成；退出码 0");

    pause_before_exit();
}

// ────────────────────────────────────────────────────────────────
// 正常模式：MultiProgress + 每步「左消息 + 右侧固定进度条」+ 底部整体进度
// ────────────────────────────────────────────────────────────────

fn normal_flow(fail: bool, spin: (usize, &'static str, &'static str)) {
    banner();

    println!();
    println!(
        "  本次动画：{}（池第 {}/{} 组，每次启动循环轮换；--spin <名字> 可指定）",
        spin.1,
        spin.0 + 1,
        spinners().len()
    );

    let tasks = tasks();
    let total_secs: f64 = tasks.iter().map(|t| t.secs).sum();
    let steps_total = tasks.len();

    let mut log = match Logger::new() {
        Ok(l) => l,
        Err(e) => {
            println!("  无法创建日志文件：{}", e);
            process::exit(1);
        }
    };
    log.w(&format!(
        "启动模式：正常（fail={}，动画={}，原型模拟）",
        fail, spin.1
    ));
    let log_path = log.path.to_string_lossy().to_string();

    let mp = MultiProgress::new();

    // 底部整体进度条：indicatif 的 active bar 恒渲染在最长行尾/底部，
    // 其余临时 bar 在 finish_and_clear 后消失，因此「整体」始终常驻底部。
    let overall = mp.add(ProgressBar::new(100));
    overall.set_style(
        ProgressStyle::with_template("  整体进度 {bar:26.cyan/green} {percent:>3}%")
            .unwrap()
            .progress_chars("━╸─"),
    );

    println!();
    println!("  （原型：以下 5 步为模拟执行，不调用真实命令）");

    let mut prior_secs = 0f64;

    for (i, task) in tasks.iter().enumerate() {
        mp.println(format!("\n  [{}/{}] {}", i + 1, steps_total, task.label))
            .ok();
        log.w(&format!(
            "[STEP {}/{}] 开始：{}",
            i + 1,
            steps_total,
            task.label
        ));

        let bar = mp.add(ProgressBar::new(100));
        bar.set_style(if task.deterministic {
            det_style()
        } else {
            spin_style(spin.2)
        });
        bar.set_message("准备中…（模拟）");
        bar.enable_steady_tick(Duration::from_millis(90));

        let outcome = simulate_and_render(
            &bar, &overall, task, prior_secs, total_secs, i, fail, &mut log,
        );
        prior_secs += task.secs;

        match outcome {
            Ok(()) => {
                bar.finish_and_clear();
                let msg = success_msg(i);
                mp.println(format!("  ✓ {}", msg)).ok();
                log.w(&format!("[STEP {}/{}] 完成：{}", i + 1, steps_total, msg));
            }
            Err(err) => {
                bar.finish_and_clear();
                overall.finish_and_clear();
                mp.println(format!("\n  ✗ {}", err)).ok();
                log.w(&format!("[STEP {}/{}] 失败：{}", i + 1, steps_total, err));
                log.w("最终退出码：4");
                mp.println("  安全相关提示：pi-web 为独立后台进程，未受本次失败影响。".to_string())
                    .ok();

                // 正常模式失败：停留按键后退出（退出码 4）
                pause_before_exit();
                process::exit(4);
            }
        }

        if i == 3 {
            // supervisor 消息样例：走同一输出层，避免与进度条打架
            mp.println(
                "  ℹ [supervisor] 常驻监控已启动（进程崩溃自动重启，最多 3 次）".to_string(),
            )
            .ok();
            log.w("supervisor 已启动");
        }
    }

    drop(overall);
    mp.println(format!("\n  📝 本次启动日志：{}", log_path))
        .ok();
    mp.println("  ✓ 全部步骤完成（退出码 0，原型）".to_string())
        .ok();
    log.w("全部步骤完成；退出码 0");

    pause_before_exit();
}

/// 单步模拟 + 双条渲染：每 100ms 推进 step bar（脉冲 / 确定进度）与整体 bar。
/// fail=true 且 idx==3 → 执行到 45% 模拟失败。
#[allow(clippy::too_many_arguments)]
fn simulate_and_render(
    bar: &ProgressBar,
    overall: &ProgressBar,
    task: &Task,
    prior_secs: f64,
    total_secs: f64,
    idx: usize,
    fail: bool,
    log: &mut Logger,
) -> Result<(), String> {
    let fails_here = fail && idx == 3;
    let effective = if fails_here {
        task.secs * 0.45
    } else {
        task.secs
    };
    let subs = sub_messages(idx);

    let start = Instant::now();
    loop {
        let el = start.elapsed().as_secs_f64().min(effective);
        let frac = if effective <= 0.0 {
            1.0
        } else {
            el / effective
        };

        if task.deterministic {
            bar.set_message("等待 pi-web HTTP 就绪…");
            bar.set_position((frac * 100.0) as u64);
        } else {
            // 无真实进度的任务：右侧固定位置 = 旋转小转圈 + 已耗时，
            // 不再使用脉冲条 / 不假装百分比
            bar.set_message("执行中…（模拟）");
        }
        // 子消息切换（如步骤 3：查询版本 → 安装中）
        for (at, m) in &subs {
            if el >= *at {
                bar.set_message(*m);
            }
        }
        overall.set_position((((prior_secs + el) / total_secs) * 100.0) as u64);

        if el >= effective {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    if fails_here {
        log.w("模拟故障：pi-web 进程在就绪前退出");
        return Err("pi-web 启动失败（模拟：端口被占用或配置有误）— 退出码 4".to_string());
    }

    Ok(())
}

fn spin_style(spin_chars: &'static str) -> ProgressStyle {
    // 右侧固定位置：{wide_msg} 把残余宽度撑满，将转圈推到行末 + 已耗时
    ProgressStyle::with_template("{wide_msg} {spinner:.green} {elapsed:>4}")
        .unwrap()
        .tick_chars(spin_chars)
}

fn det_style() -> ProgressStyle {
    ProgressStyle::with_template("{wide_msg} {bar:14.cyan/blue} {percent:>3}% eta:{eta:>3}")
        .unwrap()
        .progress_chars("━╸─")
}

// ────────────────────────────────────────────────────────────────
// 安静模式：无 UI、静默执行 + 日志、失败也自动关窗
// ────────────────────────────────────────────────────────────────

fn quiet_flow(fail: bool) {
    let mut log = match Logger::new() {
        Ok(l) => l,
        Err(e) => {
            println!("  无法创建日志文件：{}", e);
            process::exit(1);
        }
    };
    log.w(&format!(
        "启动模式：安静（--quiet，fail={}，原型模拟）",
        fail
    ));

    // 安静模式仅打印最少量信息：日志位置
    println!("  pi-launcher（--quiet 原型）：静默启动中，完成后窗口将自动关闭。");
    println!("    日志：{}", log.path.to_string_lossy());

    let tasks = tasks();
    let steps_total = tasks.len();
    for (i, task) in tasks.iter().enumerate() {
        log.w(&format!(
            "[STEP {}/{}] 开始：{}",
            i + 1,
            steps_total,
            task.label
        ));
        thread::sleep(Duration::from_secs_f64(task.secs));

        if i == 3 && fail {
            log.w("[STEP 4] 失败：pi-web 启动失败（模拟）");
            log.w("最终退出码：4（自动关窗，不停留）");
            process::exit(4); // 失败同样自动关窗
        }
        log.w(&format!("[STEP {}/{}] 完成", i + 1, steps_total));
    }

    log.w("全部步骤完成；退出码 0（pi CLI 已按安静模式策略跳过）");
}

// ────────────────────────────────────────────────────────────────
// 日志（两种模式共用；这部分为真实实现，非模拟）
// ────────────────────────────────────────────────────────────────

struct Logger {
    path: PathBuf,
    file: BufWriter<File>,
}

impl Logger {
    fn new() -> io::Result<Self> {
        let dir = Logger::dir();
        fs::create_dir_all(&dir)?;
        cleanup_logs(&dir, 20)?;

        let name = format!("pi-launcher-proto-{}.log", Logger::stamp());
        let path = dir.join(name);
        let file = File::create(&path)?;
        // 写入 UTF-8 BOM：让 PowerShell / 旧版记事本正确识别编码（否则按 ANSI 打开会乱码）
        let mut writer = BufWriter::new(file);
        let _ = writer.write_all(&[0xEF, 0xBB, 0xBF]);
        Ok(Self { path, file: writer })
    }

    fn dir() -> PathBuf {
        // 正式版将用 %LOCALAPPDATA%\pi-launcher\logs；原型用独立目录避免混淆
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("pi-launcher-proto").join("logs")
    }

    fn stamp() -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("{:012x}", secs)
    }

    fn w(&mut self, msg: &str) {
        let _ = writeln!(self.file, "{}  {}", format_ts(), msg);
        let _ = self.file.flush();
    }
}

/// 清理旧日志，仅保留最近 keep 份（文件名内含时间戳，按名排序即可）
fn cleanup_logs(dir: &Path, keep: usize) -> io::Result<usize> {
    let mut logs: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("pi-launcher-proto-") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    logs.sort();
    let total = logs.len();
    let mut removed = 0usize;
    if total > keep {
        for p in &logs[..total - keep] {
            let _ = fs::remove_file(p);
            removed += 1;
        }
    }
    Ok(removed)
}

/// 行前时间戳（原型以 UTC+8 近似；正式版改用系统时区）
fn format_ts() -> String {
    const OFFSET: i64 = 8 * 3600;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        + OFFSET;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        s / 3600,
        (s % 3600) / 60,
        s % 60
    )
}

/// Howard Hinnant 的 civil_from_days：unix 天数 → (年, 月, 日)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
