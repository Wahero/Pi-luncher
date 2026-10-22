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

use console::{style, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle, TermLike};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

pub const VERSION: &str = "1.1.5";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Quiet,
}

// ────────────────────────────────────────────────────────────────
// 显示模式（--display A|B|C）：三档交互版式，仅影响 TTY 渲染；
// 非 TTY 自动退化为纯文字、安静模式零输出，两者不受本参数影响。
//   A = 工作账（Ledger，默认）：一步一行，进行中实时刷新，完成即原位凝固
//   B = 轻修（Lite）：保留分栏仪表节奏 + 圆角细框 banner
//   C = 极简（Min）：单行流，无框无分隔线
// ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Ledger, // A：工作账
    Lite,   // B：轻修
    Min,    // C：极简
}

impl Display {
    /// 解析 --display 的值；大小写不敏感，接受简写与别名。
    pub fn from_arg(s: &str) -> Option<Display> {
        match s.to_ascii_lowercase().as_str() {
            "a" | "ledger" => Some(Display::Ledger),
            "b" | "lite" | "light" => Some(Display::Lite),
            "c" | "min" | "minimal" => Some(Display::Min),
            _ => None,
        }
    }
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
// 动画池（--quiet 不适用；每次启动循环轮换）
// ────────────────────────────────────────────────────────────────

///（用户选定的集合：dots / dots2 / grow / moon / clock / hourglass）
pub fn anim_pool() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dots", "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        ("dots2", "⣾⣽⣻⢿⡿⣟⣯⣷"),
        ("grow", "▁▃▄▅▆▇▆▅▄▃"),
        ("moon", "\u{1F311}\u{1F312}\u{1F313}\u{1F314}\u{1F315}\u{1F316}\u{1F317}\u{1F318}"),
        ("clock", "\u{1F550}\u{1F551}\u{1F552}\u{1F553}\u{1F554}\u{1F555}\u{1F556}\u{1F557}\u{1F558}\u{1F559}\u{1F55A}\u{1F55B}"),
        // 末尾多补一个帧：indicatif 把最后一个字符当「完成态」，只给 2 个字符时
        // 实际只有 1 个转动帧 → 沙漏看起来完全静止（v1.1.4 的 bug）。
        ("hourglass", "\u{23F3}\u{231B}\u{23F3}"),
    ]
}

/// --spin 指定 > 否则按「累计启动序号」循环轮换
///
/// 序号存在独立计数文件里，而不是数日志文件：日志只保留最近 `LOG_KEEP` 份，
/// 用「日志数 % 池长」在第 `LOG_KEEP + 1` 次启动起会永久停在同一个动画上。
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

    let i = next_anim_seq(list.len());
    (i, list[i].0, list[i].1)
}

/// 轮换计数文件（与 logs/ 同级，随状态一起留在 LOCALAPPDATA）
fn anim_seq_path() -> PathBuf {
    log_dir().with_file_name("anim.seq")
}

/// 读取并递增轮换序号；读写失败时退回 0（最多重复一个动画，不影响启动）。
///
/// 只存一个字节（当前池下标）: 避免为 `parse` / `to_string` 拉进数字解析与格式化代码；
/// 用 `File` 单字节读写而复用日志器的 IO 路径（`fs::read` 会引入整套 read_to_end）。
fn next_anim_seq(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let path = anim_seq_path();
    let mut byte = [0u8; 1];
    let seq = File::open(&path)
        .and_then(|mut f| f.read_exact(&mut byte))
        .ok()
        .map(|_| byte[0] as usize)
        .unwrap_or(0)
        % len;
    if let Ok(mut f) = File::create(&path) {
        let _ = f.write_all(&[((seq + 1) % len) as u8]);
    }
    seq
}

// ────────────────────────────────────────────────────────────────
// 通知通道（跨线程；supervisor 用，不携带进度条状态）
// ────────────────────────────────────────────────────────────────

struct Notify {
    mp: Option<MultiProgress>,
    plain_text: bool,
    quiet: bool,
}

/// `--force-tty` 用的假终端。
///
/// indicatif 会在两处检查 `is_term()`，非终端时**整块渲染静默丢弃**（连 println 都不出），
/// 单纯置一个 `force_tty = true` 并不会真的画出动画。这里固定报告「是终端」，
/// 把帧原样写到 stdout，供无 TTY 环境（CI、重定向、抓帧）验证动画与排版。
#[derive(Debug)]
struct ForcedTerm;

/// 强制终端：不做缓冲，逐帧直写 stdout（抓帧时顺序即可读）。
/// 十进制手动拼，避免每帧一次 `format!` 分配（发布版体积敏感）。
fn forced_csi(n: usize, final_byte: u8) -> io::Result<()> {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    let mut v = n;
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let mut out = io::stdout().lock();
    out.write_all(b"\x1b[")?;
    out.write_all(&digits[i..])?;
    out.write_all(&[final_byte])?;
    out.flush()
}

impl TermLike for ForcedTerm {
    fn width(&self) -> u16 {
        100
    }

    fn height(&self) -> u16 {
        30
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        forced_csi(n, b'A')
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        forced_csi(n, b'B')
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        forced_csi(n, b'C')
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        forced_csi(n, b'D')
    }

    fn write_line(&self, s: &str) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(s.as_bytes())?;
        out.write_all(b"\n")?;
        out.flush()
    }

    fn write_str(&self, s: &str) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(s.as_bytes())?;
        out.flush()
    }

    fn clear_line(&self) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(b"\r\x1b[2K")?;
        out.flush()
    }

    fn flush(&self) -> io::Result<()> {
        io::stdout().flush()
    }
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
// 前景 UI：步骤渲染（交互 / 纯文字 / 安静模式）
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
    display: Display,
    steps_total: usize,
    anim_name: Option<&'static str>,
    anim_index: Option<usize>,

    // 底部整体进度（仅交互模式）：初始化时最先 add，所有步骤行都插入在它上方，
    // 因此它始终钉在屏幕最底部
    overall: Option<ProgressBar>,
    overall_base: u64, // 已完成步骤的累计百分比基数

    // 当前步骤（流程串行，同一步骤的 bar / 计时全局唯一）
    current_bar: Option<ProgressBar>,
    current_det: bool,
    current_idx: usize,
    current_row_label: String,
    step_started: Option<Instant>,

    // 已凝固的步骤行（行即事实）：成功收尾时先撤场再原样重放，
    // 让收尾信息排在账目最后一行之后（println 会插在渲染块上方）
    frozen_bars: Vec<ProgressBar>,
    frozen_rows: Vec<String>,
}

impl Ui {
    /// 建立输出层（日志器已在 main 先行初始化）
    pub fn init(
        mode: Mode,
        display: Display,
        force_tty: bool,
        spin_forced: Option<&str>,
        steps_total: usize,
    ) -> Ui {
        match mode {
            Mode::Quiet => {
                let _ = NOTIFY.set(Notify {
                    mp: None,
                    plain_text: false,
                    quiet: true,
                });
                Ui {
                    kind: Kind::Quiet,
                    display,
                    steps_total,
                    anim_name: None,
                    anim_index: None,
                    overall: None,
                    overall_base: 0,
                    current_bar: None,
                    current_det: false,
                    current_idx: 0,
                    current_row_label: String::new(),
                    step_started: None,
                    frozen_bars: Vec::new(),
                    frozen_rows: Vec::new(),
                }
            }
            Mode::Normal => {
                let tty = force_tty || Term::stdout().is_term();
                if !tty {
                    log("显示层：非交互终端（stdout 非终端）→ 纯文字，不渲染动画；--force-tty 可强制动画渲染");
                    println!("  ℹ 非交互终端：动画渲染已关闭（在终端窗口直接运行，或加 --force-tty 可看到动画）");
                    let _ = NOTIFY.set(Notify {
                        mp: None,
                        plain_text: true,
                        quiet: false,
                    });
                    return Ui {
                        kind: Kind::Text,
                        display,
                        steps_total,
                        anim_name: None,
                        anim_index: None,
                        overall: None,
                        overall_base: 0,
                        current_bar: None,
                        current_det: false,
                        current_idx: 0,
                        current_row_label: String::new(),
                        step_started: None,
                        frozen_bars: Vec::new(),
                        frozen_rows: Vec::new(),
                    };
                }

                let (idx, name, chars) = pick_anim(spin_forced);
                log(&format!(
                    "动画主题：{}（池 {}/{} 组；--spin <名字> 可指定）",
                    name,
                    idx + 1,
                    anim_pool().len()
                ));
                log(&format!(
                    "显示模式：{}（--display {}）",
                    display_label(display),
                    "A|B|C"
                ));
                // 画到哪，就以哪判断终端：判定读的是 stdout，渲染也必须落在 stdout。
                // indicatif 默认画 stderr，一旦 stderr 被重定向，draw target 会静默隐藏，
                // 出现「判定说是终端，屏幕上一个字都没有」的情况（v1.1.3 之前的事故）。
                let target = if force_tty {
                    ProgressDrawTarget::term_like_with_hz(Box::new(ForcedTerm), 20)
                } else {
                    ProgressDrawTarget::stdout()
                };
                let mp = MultiProgress::with_draw_target(target);

                // 底部整体进度条：整体是第一个加入的 bar（占 index 0），
                // 后续步骤行通过 insert(idx-1) 插到它上方 → 整体永远钉在底部
                let overall = mp.add(ProgressBar::new(100));
                overall.set_style(overall_style(display));

                // supervisor 等后台线程需要同一条 MultiProgress 来安全输出
                let _ = NOTIFY.set(Notify {
                    mp: Some(mp.clone()),
                    plain_text: false,
                    quiet: false,
                });

                Ui {
                    kind: Kind::Interactive { mp, spin_chars: chars },
                    display,
                    steps_total,
                    anim_name: Some(name),
                    anim_index: Some(idx),
                    overall: Some(overall),
                    overall_base: 0,
                    current_bar: None,
                    current_det: false,
                    current_idx: 0,
                    current_row_label: String::new(),
                    step_started: None,
                    frozen_bars: Vec::new(),
                    frozen_rows: Vec::new(),
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

    /// 步骤开始：按模式插入一行（insert 确保步骤行都排在整体条上方 → 整体钉底）。
    /// det → 确定进度条；spin → 转圈。日志仍记长标题，行内用短名。
    pub fn step_start(&mut self, idx: usize, label: &str, row_label: &str, det: bool) {
        let total = self.steps_total;
        log(&format!("[STEP {}/{}] 开始：{}", idx, total, label));

        self.current_idx = idx;
        self.current_det = det;
        self.step_started = Some(Instant::now());
        self.current_bar = None;
        self.current_row_label = row_label.to_string();

        match &self.kind {
            Kind::Interactive { mp, spin_chars } => {
                let bar = mp.insert(idx.saturating_sub(1), ProgressBar::new(100));
                let tpl = run_tpl(self.display, det);
                let sty = if det {
                    ProgressStyle::with_template(tpl)
                        .expect("步骤行模板是静态合法模板")
                        .progress_chars("━╸─")
                } else {
                    ProgressStyle::with_template(tpl)
                        .expect("步骤行模板是静态合法模板")
                        .tick_chars(spin_chars)
                };
                bar.set_style(sty);
                bar.set_prefix(self.running_prefix());
                // spin 行右侧动作位初始为「执行中…」，避免 wide_msg 重复显示步骤名；
                // det 行模板不含 {msg}，置空即可
                bar.set_message(if det {
                    String::new()
                } else {
                    "执行中…".to_string()
                });
                bar.enable_steady_tick(std::time::Duration::from_millis(90));
                self.current_bar = Some(bar);
            }
            Kind::Text => {
                println!("\n  [{}/{}] {}", idx, total, label);
            }
            Kind::Quiet => {}
        }
    }

    /// 更新当前步骤右侧动态动作（仅 spin 行有 {wide_msg}；det 行右侧被 bar 占用）
    pub fn step_message(&mut self, msg: &str) {
        if let Kind::Interactive { .. } = &self.kind {
            if !self.current_det {
                if let Some(bar) = &self.current_bar {
                    bar.set_message(msg.to_string());
                }
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

    /// 步骤成功：行原位凝固为 ✓ 行；整体进度推进一个步距
    pub fn step_ok(&mut self, msg: &str) {
        self.frozen_step(true, msg);
    }

    /// 步骤失败：行原位凝固为 ✗ 行
    pub fn step_fail(&mut self, msg: &str) {
        self.frozen_step(false, msg);
    }

    /// 白盒内部：把当前活动行替换为凝固行（不换行、不再刷新）。
    /// 行文本同时写入 frozen_rows，成功收尾时 replay_ledger 会原样重放。
    fn frozen_step(&mut self, ok: bool, msg: &str) {
        let secs = self.step_secs();
        let idx = self.current_idx;
        let per = 100u64 / self.steps_total.max(1) as u64;

        log(&format!(
            "[STEP {}/{}] {}：{}（{:.1}s）",
            idx,
            self.steps_total,
            if ok { "完成" } else { "失败" },
            msg,
            secs
        ));

        let interactive = matches!(&self.kind, Kind::Interactive { .. });
        if let Some(bar) = self.current_bar.take() {
            if interactive {
                let line = self.frozen_line(ok, msg, secs);
                bar.set_style(
                    ProgressStyle::with_template("{msg}").expect("模板 '{msg}' 是合法模板"),
                );
                bar.set_message(line.clone());
                bar.finish();
                self.frozen_bars.push(bar);
                self.frozen_rows.push(line);
            } else {
                bar.finish_and_clear();
            }
        } else if interactive {
            // 步骤行已交接为纯文本（如 Pi CLI 交互前后）：无 bar 可凝固，直接落普通行
            let line = self.frozen_line(ok, msg, secs);
            if let Kind::Interactive { mp, .. } = &self.kind {
                mp.println(line).ok();
            }
        }
        self.step_started = None;

        match &self.kind {
            Kind::Interactive { .. } => {
                if ok {
                    if let Some(overall) = &self.overall {
                        self.overall_base = ((self.overall_base / per) + 1) * per;
                        overall.set_position(self.overall_base);
                    }
                }
            }
            Kind::Text => {
                if ok {
                    println!("  ✓ {}（{:.1}s）", msg, secs);
                } else {
                    println!("  ✗ {}", msg);
                }
            }
            Kind::Quiet => {}
        }
    }

    /// 渲染交接：撤掉全部活体渲染（底部整体条 + 当前步骤行 + 已凝固行），
    /// 再把凝固行原样重放为普通行。三处调用：
    /// - 交出屏幕给 pi TUI 前（步骤 5）：pi 的 inline TUI 与任何还在重绘的 bar
    ///   都会互相糊画面（v1.1.0 回归事故），交接后本层只允许纯 println；
    /// - 成功收尾：让日志路径 / ✓ 全部步骤完成排在账目行之后；
    /// - 失败（步骤 1-4）：账目保持可见，只是从活体渲染定格为纯文本，
    ///   收尾倒数也因此走纯 stdout，与游标不再有任何竞争。
    pub fn replay_ledger(&mut self) {
        let is_interactive = matches!(&self.kind, Kind::Interactive { .. });
        if !is_interactive {
            return;
        }

        // 字段级借用互相独立，逐个处理
        if let Some(overall) = self.overall.take() {
            overall.finish_and_clear();
        }
        if let Some(bar) = self.current_bar.take() {
            bar.finish_and_clear();
        }
        let bars = std::mem::take(&mut self.frozen_bars);
        for bar in bars {
            bar.finish_and_clear();
        }
        let rows = std::mem::take(&mut self.frozen_rows);
        if let Kind::Interactive { mp, .. } = &self.kind {
            for row in rows {
                mp.println(row).ok();
            }
        }
    }

    fn step_secs(&self) -> f64 {
        self.step_started
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }

    /// 运行中的行前缀：编号（上色）+ 名称（按显示宽度补位）。
    /// 与冻结行共用同一套编号/名称规则，保证行切换前后语义一致。
    fn running_prefix(&self) -> String {
        let idx = self.current_idx;
        let (num, nw) = match self.display {
            Display::Ledger => (format!("{}/{}", idx, self.steps_total), 20),
            Display::Lite => (format!("[{}/{}]", idx, self.steps_total), 20),
            Display::Min => (format!("{}·", idx), 7),
        };
        format!(
            "{} {}",
            style(num).cyan(),
            pad_width(self.current_row_label.as_str(), nw)
        )
    }

    /// 凝固行文本：编号（灰）+ 名称 + ✓/✗ + 耗时 + 结论。
    fn frozen_line(&self, ok: bool, msg: &str, secs: f64) -> String {
        let idx = self.current_idx;
        let (num, nw) = match self.display {
            Display::Ledger => (format!("{}/{}", idx, self.steps_total), 20),
            Display::Lite => (format!("[{}/{}]", idx, self.steps_total), 20),
            Display::Min => (format!("{}·", idx), 7),
        };
        let mark = if ok { style("✓").green() } else { style("✗").red() };
        let tail = if ok { style(msg).dim() } else { style(msg).red() };
        format!(
            "  {} {} {} {} {}",
            style(num).dim(),
            pad_width(self.current_row_label.as_str(), nw),
            mark,
            style(format!("{:.1}s", secs)).dim(),
            tail
        )
    }
}

// ────────────────────────────────────────────────────────────────
// 模板与文本工具（展示宽度全部按 unicode-width 口径计算）
// ────────────────────────────────────────────────────────────────

/// 显示模式下的人类可读名（日志用）
fn display_label(d: Display) -> &'static str {
    match d {
        Display::Ledger => "工作账",
        Display::Lite => "轻修",
        Display::Min => "极简",
    }
}

/// 交互步骤行模板（80 列预算内；{prefix} 为「编号+名称」整体，
/// {wide_msg}/{bar} 是右侧动态动作位）。
fn run_tpl(display: Display, det: bool) -> &'static str {
    match (display, det) {
        // A · 工作账：编号+名称 25 列 | 转圈 | 计时 | 右侧动作
        (Display::Ledger, false) => {
            "  {prefix:<25} {spinner:.cyan} {elapsed:>4.dim} {wide_msg:.dim}"
        }
        (Display::Ledger, true) => {
            "  {prefix:<25} {elapsed:>4.dim} {bar:10.cyan/blue} {percent:>3}% eta:{eta:>2}"
        }
        // B · 轻修：编号+名称 26 列 + 短竖线分隔 + 状态
        (Display::Lite, false) => {
            "  {prefix:<26} ─────────────── {spinner:.cyan} {elapsed:>4.dim} {wide_msg:.dim}"
        }
        (Display::Lite, true) => {
            "  {prefix:<26} ─────────────── {elapsed:>4.dim} {bar:8.cyan/blue} {percent:>3}% eta:{eta:>2}"
        }
        // C · 极简：10 列短行
        (Display::Min, false) => "  {prefix:<10} {spinner:.cyan} {elapsed:>4.dim} {wide_msg:.dim}",
        (Display::Min, true) => {
            "  {prefix:<10} {elapsed:>4.dim} {bar:8.cyan/blue} {percent:>3}%"
        }
    }
}

/// 底部整体条模板（C 档收窄宽度）
fn overall_tpl(display: Display) -> &'static str {
    match display {
        Display::Min => "  整体 {bar:16.cyan/green} {percent:>3}%",
        _ => "  整体进度 {bar:26.cyan/green} {percent:>3}%",
    }
}

/// 底部整体条样式
fn overall_style(display: Display) -> ProgressStyle {
    ProgressStyle::with_template(overall_tpl(display))
        .expect("整体条模板是静态合法模板")
        .progress_chars("━╸─")
}

/// 按显示宽度把字符串右补空格到指定列宽（超宽则原样返回，交由 wide_msg/终端裁剪）
pub fn pad_width(s: &str, w: usize) -> String {
    let d = UnicodeWidthStr::width(s);
    if d >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_arg_aliases() {
        assert_eq!(Display::from_arg("a"), Some(Display::Ledger));
        assert_eq!(Display::from_arg("A"), Some(Display::Ledger));
        assert_eq!(Display::from_arg("ledger"), Some(Display::Ledger));
        assert_eq!(Display::from_arg("b"), Some(Display::Lite));
        assert_eq!(Display::from_arg("lite"), Some(Display::Lite));
        assert_eq!(Display::from_arg("C"), Some(Display::Min));
        assert_eq!(Display::from_arg("minimal"), Some(Display::Min));
        assert_eq!(Display::from_arg("x"), None);
        assert_eq!(Display::from_arg(""), None);
    }

    #[test]
    fn pad_width_arena() {
        assert_eq!(pad_width("pi", 6), "pi    ");
        let padded = pad_width("检查并升级 pi-web", 20);
        assert_eq!(UnicodeWidthStr::width(padded.as_str()), 20);
        // 超宽不额外截断：交给 wide_msg / 终端负责
        let wide = pad_width("pi-web1234567890123456", 20);
        assert_eq!(UnicodeWidthStr::width(wide.as_str()), 22);
    }

    #[test]
    fn anim_pool_has_two_spinning_frames() {
        // indicatif 的 tick_chars 把最后一个字符留作「完成态」，
        // 实际参与转动的帧数 = 字符数 - 1。少于 2 帧时转圈看起来是静止的
        // （v1.1.4 的 hourglass 只有 ⏳⌛ 两个字符 → 永远停在 ⏳）。
        for (name, chars) in anim_pool() {
            let n = chars.chars().count();
            assert!(n >= 3, "动画 {name} 只有 {n} 个字符，转动帧不足 2 个");
        }
    }

    #[test]
    fn all_row_templates_parse() {
        // 六张步骤行模板 + 三张整体条模板必须全部合法，避免运行期 panic
        for d in [Display::Ledger, Display::Lite, Display::Min] {
            for det in [false, true] {
                let tpl = run_tpl(d, det);
                let sty = ProgressStyle::with_template(tpl)
                    .unwrap_or_else(|e| panic!("模板解析失败 {tpl:?}: {e}"));
                let _ = if det {
                    sty.progress_chars("━╸─")
                } else {
                    sty.tick_chars("⠋⠙")
                };
            }
            let _ = overall_style(d);
        }
    }

    #[test]
    fn cli_handoff_replays_and_clears_render() {
        // v1.1.0 回归：pi TUI 期间残留活体渲染（整体条 + 凝固行 + 当前行带 90ms
        // 定时重绘）→ 与 inline TUI 互相糊画面、账目行重复打印。
        // handoff 契约：replay_ledger 后不得残留任何 bar 状态；
        // 交接后 step_ok / step_fail 无 bar 也不得 panic。
        for d in [Display::Ledger, Display::Lite, Display::Min] {
            let mut ui = Ui::init(Mode::Normal, d, true, None, 5);
            ui.step_start(1, "环境预检", "环境预检", false);
            ui.step_ok("就绪");
            ui.step_start(2, "更新", "pi update --all", false);
            ui.step_progress(0.4);
            ui.step_ok("完成");

            ui.step_start(5, "CLI", "Pi CLI", false);
            ui.replay_ledger(); // 交接：全部撤场 + 账目转纯文本
            assert!(ui.overall.is_none(), "{d:?}: 整体条未撤场");
            assert!(ui.current_bar.is_none(), "{d:?}: 当前行未撤场");
            assert!(ui.frozen_bars.is_empty(), "{d:?}: 凝固 bar 未撤场");
            assert!(ui.frozen_rows.is_empty(), "{d:?}: 账目行未重放");

            ui.step_ok("Pi CLI 已正常退出"); // 无 bar 路径，不得 panic
        }
        // 失败路径同样交接后收尾
        let mut ui = Ui::init(Mode::Normal, Display::Ledger, true, None, 5);
        ui.step_start(2, "更新", "pi update --all", false);
        ui.step_fail("模拟失败");
        ui.replay_ledger();
        assert!(ui.current_bar.is_none());
        assert!(ui.frozen_bars.is_empty());
    }
}
