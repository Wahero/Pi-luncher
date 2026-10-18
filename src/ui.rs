////////////////////////////////////////////////////////////////////////////////
//! 统一输出层：控制台渲染 + 全程文字日志。
//!
//! 所有步骤的打印与日志必须经由本模块，保证：
//! - 交互模式（TTY 进度动画）：步骤行「左消息 + 右侧固定位置动画/进度条」，
//!   底部常驻整体进度；普通行输出用 MultiProgress::println，避免与重绘打架；
//! - 非 TTY（管道/CI）：纯文字退化（println），无渲染开销；
//! - 安静模式（--quiet）：控制台零输出，但日志照常记录。
//!
//! 日志（两种模式均写）：%LOCALAPPDATA%\pi-launcher\logs\pi-launcher-<时间戳>.log
//! UTF-8（带 BOM，避免旧版记事本/PowerShell 按 ANSI 打开乱码），
//! 每次启动建新文件，自动保留最近 LOG_KEEP 份。
////////////////////////////////////////////////////////////////////////////////

use console::Term;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub const VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Quiet,
}

pub const LOG_KEEP: usize = 20;
const LOG_PREFIX: &str = "pi-launcher-";

// ────────────────────────────────────────────────────────────────
// 日志（真实实现；两种模式共用）
// ────────────────────────────────────────────────────────────────

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

struct Logger {
    path: PathBuf,
    file: BufWriter<File>,
}

impl Logger {
    fn new() -> io::Result<Self> {
        let dir = log_dir();
        fs::create_dir_all(&dir)?;
        cleanup_logs(&dir, LOG_KEEP)?;

        let path = dir.join(format!("{}{}.log", LOG_PREFIX, stamp_hex()));
        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);
        // UTF-8 BOM：让 PowerShell / 旧版记事本正确识别编码
        let _ = writer.write_all(&[0xEF, 0xBB, 0xBF]);
        Ok(Self { path, file: writer })
    }

    fn write(&mut self, msg: &str) {
        let _ = writeln!(self.file, "{}  {}", local_time_string(), msg);
        let _ = self.file.flush();
    }
}

/// 日志目录：正式版用 %LOCALAPPDATA%\pi-launcher\logs
pub fn log_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("pi-launcher").join("logs")
}

/// 清理旧日志，仅保留最近 keep 份（文件名含时间戳，按名排序即可）
fn cleanup_logs(dir: &Path, keep: usize) -> io::Result<usize> {
    let mut logs: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(LOG_PREFIX) && n.ends_with(".log"))
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

fn stamp_hex() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{:012x}", secs)
}

/// 初始化全局日志器（两种模式都在 main 里最先调用）。
/// 返回日志文件完整路径。
pub fn init_logger() -> io::Result<String> {
    let logger = Logger::new()?;
    let path = logger.path.to_string_lossy().to_string();
    let _ = LOGGER.set(Mutex::new(logger));
    Ok(path)
}

/// 任意线程可用的日志输出（时间戳自动附加）
pub fn log(msg: &str) {
    if let Some(m) = LOGGER.get() {
        if let Ok(mut l) = m.lock() {
            l.write(msg);
        }
    }
}

/// 当前日志文件路径（未初始化时返回占位）
pub fn log_path() -> String {
    match LOGGER.get().and_then(|m| m.lock().ok()) {
        Some(l) => l.path.to_string_lossy().to_string(),
        None => "(日志未初始化)".to_string(),
    }
}

/// 行前时间戳：Windows 用系统本地时间；其它平台退化为 UTC
#[cfg(windows)]
pub fn local_time_string() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let mut st = SYSTEMTIME {
            wYear: 0,
            wMonth: 0,
            wDayOfWeek: 0,
            wDay: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        GetLocalTime(&mut st);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
}

#[cfg(not(windows))]
pub fn local_time_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
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
#[cfg(not(windows))]
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

// ────────────────────────────────────────────────────────────────
// 动画池（--quiet 不适用；每次启动按日志数循环轮换）
// ────────────────────────────────────────────────────────────────

///（用户选定的集合：dots / dots2 / growVertical / moon / clock / hourglass）
pub fn anim_pool() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dots", "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        ("dots2", "⣾⣽⣻⢿⡿⣟⣯⣷"),
        ("grow", "▁▃▄▅▆▇▆▅▄▃"),
        ("moon", "\u{1F311}\u{1F312}\u{1F313}\u{1F314}\u{1F315}\u{1F316}\u{1F317}\u{1F318}"),
        ("clock", "\u{1F550}\u{1F551}\u{1F552}\u{1F553}\u{1F554}\u{1F555}\u{1F556}\u{1F557}\u{1F558}\u{1F559}\u{1F55A}\u{1F55B}"),
        ("hourglass", "\u{23F3}\u{231B}"),
    ]
}

/// --spin 指定 > 否则按「本机已有日志数」循环轮换
/// （每次启动产生一个新日志文件，日志数即启动次数，天然形成循环）
pub fn pick_anim(forced: Option<&str>) -> (usize, &'static str, &'static str) {
    let list = anim_pool();

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
    fs::read_dir(log_dir())
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.starts_with(LOG_PREFIX) && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

// ────────────────────────────────────────────────────────────────
// 通知通道（跨线程；supervisor 用，不携带进度条状态）
// ────────────────────────────────────────────────────────────────

struct Notify {
    mp: Option<MultiProgress>,
    plain_text: bool,
    quiet: bool,
}

static NOTIFY: OnceLock<Notify> = OnceLock::new();

/// 后台线程（supervisor 等）用的提示：打印（安静模式除外）+ 日志
pub fn notify(msg: &str) {
    log(msg);
    match NOTIFY.get() {
        None => println!("{}", msg),
        Some(n) if n.quiet => {}
        Some(n) if n.plain_text => println!("{}", msg),
        Some(n) => {
            if let Some(mp) = &n.mp {
                mp.println(msg).ok();
            } else {
                println!("{}", msg);
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────
// 前景 UI：步骤渲染（交互 / 纯文字 / 安静 郑州）
// ────────────────────────────────────────────────────────────────

enum Kind {
    /// TTY 动画渲染：MultiProgress + 右侧固定动画/进度条 + 底部整体进度
    Interactive {
        mp: MultiProgress,
        spin_chars: &'static str,
    },
    /// 非 TTY：纯文字
    Text,
    /// --quiet：零输出（日志在 log() 内照常写）
    Quiet,
}

pub struct Ui {
    kind: Kind,
    steps_total: usize,
    anim_name: Option<&'static str>,
    anim_index: Option<usize>,

    // 底部整体进度（仅交互模式）
    overall: Option<ProgressBar>,
    overall_base: u64, // 已完成步骤的累计百分比基数

    // 当前步骤（流程串行，同一步骤的 bar / 计时全局唯一）
    current_bar: Option<ProgressBar>,
    current_det: bool,
    current_idx: usize,
    step_started: Option<Instant>,
}

impl Ui {
    /// 建立输出层（日志器已在 main 先行初始化）
    pub fn init(mode: Mode, force_tty: bool, spin_forced: Option<&str>, steps_total: usize) -> Ui {
        match mode {
            Mode::Quiet => {
                let _ = NOTIFY.set(Notify {
                    mp: None,
                    plain_text: false,
                    quiet: true,
                });
                Ui {
                    kind: Kind::Quiet,
                    steps_total,
                    anim_name: None,
                    anim_index: None,
                    overall: None,
                    overall_base: 0,
                    current_bar: None,
                    current_det: false,
                    current_idx: 0,
                    step_started: None,
                }
            }
            Mode::Normal => {
                let tty = force_tty || Term::stdout().is_term();
                if !tty {
                    let _ = NOTIFY.set(Notify {
                        mp: None,
                        plain_text: true,
                        quiet: false,
                    });
                    return Ui {
                        kind: Kind::Text,
                        steps_total,
                        anim_name: None,
                        anim_index: None,
                        overall: None,
                        overall_base: 0,
                        current_bar: None,
                        current_det: false,
                        current_idx: 0,
                        step_started: None,
                    };
                }

                let (idx, name, chars) = pick_anim(spin_forced);
                log(&format!(
                    "动画主题：{}（池 {}/{} 组；--spin <名字> 可指定）",
                    name,
                    idx + 1,
                    anim_pool().len()
                ));
                let mp = MultiProgress::new();

                // 底部整体进度条：之后加进来的步骤 bar 渲染在它下方，
                // 步骤 bar finish_and_clear 后消失，「整体」常驻底部。
                let overall = mp.add(ProgressBar::new(100));
                overall.set_style(
                    ProgressStyle::with_template("  整体进度 {bar:26.cyan/green} {percent:>3}%")
                        .unwrap()
                        .progress_chars("━╸─"),
                );

                // supervisor 等后台线程需要同一条 MultiProgress 来安全输出
                let _ = NOTIFY.set(Notify {
                    mp: Some(mp.clone()),
                    plain_text: false,
                    quiet: false,
                });

                Ui {
                    kind: Kind::Interactive {
                        mp,
                        spin_chars: chars,
                    },
                    steps_total,
                    anim_name: Some(name),
                    anim_index: Some(idx),
                    overall: Some(overall),
                    overall_base: 0,
                    current_bar: None,
                    current_det: false,
                    current_idx: 0,
                    step_started: None,
                }
            }
        }
    }

    /// 本次动画名（交互模式才有；供 banner 展示轮换结果）
    pub fn anim_label(&self) -> Option<(usize, &'static str)> {
        match (&self.anim_index, &self.anim_name) {
            (Some(i), Some(n)) => Some((*i, *n)),
            _ => None,
        }
    }

    /// 通用一行（含 ✓/✗/⚠/ℹ 前缀均由调用方给出）：打印（安静模式除外）+ 落盘
    pub fn message(&self, msg: &str) {
        log(msg);
        match &self.kind {
            Kind::Interactive { mp, .. } => {
                mp.println(msg).ok();
            }
            Kind::Text => println!("{}", msg),
            Kind::Quiet => {}
        }
    }

    /// stderr 摘要打印（供失败步骤复用）：缩进显示每行
    pub fn print_stderr_summary(&self, stderr: &str) {
        if stderr.is_empty() {
            return;
        }
        self.message("  错误摘要：");
        for line in stderr.lines() {
            self.message(&format!("    {}", line));
        }
    }

    /// 步骤开始：打印步骤标题、建 bar（det → 确定进度条；spin → 右侧转圈）并记日志
    pub fn step_start(&mut self, idx: usize, label: &str, det: bool) {
        let total = self.steps_total;
        log(&format!("[STEP {}/{}] 开始：{}", idx, total, label));

        self.current_idx = idx;
        self.current_det = det;
        self.step_started = Some(Instant::now());
        self.current_bar = None;

        match &self.kind {
            Kind::Interactive { mp, .. } => {
                mp.println(format!("\n  [{}/{}] {}", idx, total, label))
                    .ok();

                let bar = mp.add(ProgressBar::new(100));
                let style = if det {
                    det_style()
                } else {
                    spin_style(self.spin_chars())
                };
                bar.set_style(style);
                bar.set_message(if det { "等待中…" } else { "执行中…" });
                bar.enable_steady_tick(std::time::Duration::from_millis(90));
                self.current_bar = Some(bar);
            }
            Kind::Text => {
                println!("\n  [{}/{}] {}", idx, total, label);
            }
            Kind::Quiet => {}
        }
    }

    /// 更新当前步骤的 bar 右侧消息
    pub fn step_message(&mut self, msg: &str) {
        if let Kind::Interactive { .. } = &self.kind {
            if let Some(bar) = &self.current_bar {
                bar.set_message(msg.to_string());
            }
        }
    }

    /// 确定进度（0.0-1.0）：更新当前步骤 bar + 整体进度条按窗口爬升
    pub fn step_progress(&mut self, frac: f64) {
        if !self.current_det {
            return;
        }
        let per = 100u64 / self.steps_total.max(1) as u64;
        if let Some(bar) = &self.current_bar {
            bar.set_position((frac * 100.0) as u64);
        }
        if let Some(overall) = &self.overall {
            let creep = (frac.clamp(0.0, 1.0) * per as f64) as u64;
            overall.set_position((self.overall_base + creep).min(self.overall_base + per));
        }
    }

    /// 步骤成功：清 bar、打印 ✓、日志带耗时；整体进度推进一个步距
    pub fn step_ok(&mut self, msg: &str) {
        let secs = self.step_secs();
        let idx = self.current_idx;
        let per = 100u64 / self.steps_total.max(1) as u64;

        log(&format!(
            "[STEP {}/{}] 完成：{}（{:.1}s）",
            idx, self.steps_total, msg, secs
        ));

        if let Some(bar) = self.current_bar.take() {
            bar.finish_and_clear();
        }
        self.step_started = None;

        match &self.kind {
            Kind::Interactive { mp, .. } => {
                mp.println(format!("  ✓ {}", msg)).ok();
                if let Some(overall) = &self.overall {
                    self.overall_base = ((self.overall_base / per) + 1) * per;
                    overall.set_position(self.overall_base);
                }
            }
            Kind::Text => println!("  ✓ {}（{:.1}s）", msg, secs),
            Kind::Quiet => {}
        }
    }

    /// 步骤失败：清 bar、打印 ✗、落盘
    pub fn step_fail(&mut self, msg: &str) {
        let secs = self.step_secs();
        let idx = self.current_idx;
        log(&format!(
            "[STEP {}/{}] 失败：{}（{:.1}s）",
            idx, self.steps_total, msg, secs
        ));

        if let Some(bar) = self.current_bar.take() {
            bar.finish_and_clear();
        }
        self.step_started = None;

        match &self.kind {
            Kind::Interactive { mp, .. } => {
                mp.println(format!("  ✗ {}", msg)).ok();
            }
            Kind::Text => println!("  ✗ {}", msg),
            Kind::Quiet => {}
        }
    }

    /// 启动 pi CLI 前清掉所有剩余渲染（TUI 与 bar 重绘会互相打架）
    pub fn overall_finish(&mut self) {
        if let Some(overall) = self.overall.take() {
            overall.finish_and_clear();
        }
    }

    fn spin_chars(&self) -> &'static str {
        match &self.kind {
            Kind::Interactive { spin_chars, .. } => spin_chars,
            _ => "",
        }
    }

    fn step_secs(&self) -> f64 {
        self.step_started
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
}

fn spin_style(spin_chars: &'static str) -> ProgressStyle {
    // 右侧固定位置：{wide_msg} 把残余宽度撑满，将转圈 + 已耗时推到行末
    ProgressStyle::with_template("{wide_msg} {spinner:.green} {elapsed:>4}")
        .unwrap()
        .tick_chars(spin_chars)
}

fn det_style() -> ProgressStyle {
    ProgressStyle::with_template("{wide_msg} {bar:14.cyan/blue} {percent:>3}% eta:{eta:>3}")
        .unwrap()
        .progress_chars("━╸─")
}
