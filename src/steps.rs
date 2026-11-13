use crate::command_runner::{
    build_command, command_exists, probe_pi_web, run_capture, run_checked, spawn_detached,
    terminate_background, wait_ready, RunResult, StartupOutcome, WebProbe,
};
use crate::ui::{self, Ui};
use std::path::{Path, PathBuf};
use std::process::Child;

/// 启动步骤总数（与 main 的流程一一对应）
pub const TOTAL_STEPS: usize = 5;

/// pi-web 就绪探测的等待上限（秒）。首次启动可能要下载/编译，给足时间。
const READY_TIMEOUT_SECS: u64 = 60;

/// pi-web 服务包名（npm 发起升级时使用）
const PI_WEB_PKG: &str = "@agegr/pi-web";
/// 升级命令的版本标签（始终安装 registry 上的最新版）
const PI_WEB_PKG_LATEST: &str = "@agegr/pi-web@latest";
/// 包内 CLI 入口（相对 npm 全局前缀），用于 shim 缺失时自行补写。
const PI_WEB_ENTRY: &str = "node_modules/@agegr/pi-web/bin/pi-web.js";
/// Windows：STATUS_CONTROL_C_EXIT（npm 升级被 Ctrl+C / 关窗打断）
const CTRL_C_EXIT_CODE: i32 = -1073741510; // 0xC000013A

/// 步骤 4 的结果：本次 spawn 的 pi-web 交给 supervisor 监控；
/// 已在运行的（此前脱离启动的实例）无从监控，跳过即可。
#[derive(Debug)]
pub enum WebStart {
    Spawned(Child),
    AlreadyRunning,
}

/// `pi` 命令所属的 npm 包（Pi Coding Agent 本体）
const PI_INSTALL_CMD: &str = "npm install -g --ignore-scripts @earendil-works/pi-coding-agent";
/// `pi-web` 命令所属的 npm 包：与 `pi` 是两个独立包，装错包补不出 `pi-web`。
const PI_WEB_INSTALL_CMD: &str = "npm install -g @agegr/pi-web";
/// `npm` 缺失只能装 Node.js，没有 npm 命令可用
const NPM_INSTALL_CMD: &str = "https://nodejs.org/ 安装 Node.js（装完重开终端）";

/// 缺失命令对应的安装提示：每个命令来自不同来源，不能共用一条提示。
fn install_hint(name: &str) -> &'static str {
    match name {
        "pi" => PI_INSTALL_CMD,
        "pi-web" => PI_WEB_INSTALL_CMD,
        _ => NPM_INSTALL_CMD,
    }
}

/// 步骤 1：环境预检（pi / pi-web / npm）
/// 行内只显示一行（右上侧动态报当前检查项），逐项就绪写进日志。
/// `pi-web` 缺失时先尝试自动修复（npm 升级中断留下的残留 shim / 缺 shim），再判定失败。
pub fn step_preflight(ui: &mut Ui) -> Result<(), String> {
    ui.step_start(1, "环境预检（pi / pi-web / npm）", "环境预检", false);

    let required = ["pi", "pi-web", "npm"];
    for name in &required {
        ui.step_message(&format!("检查命令 `{}` …", name));
        let mut found = command_exists(name);

        if !found && *name == "pi-web" {
            ui.message("  ⚠ 未找到 `pi-web`，尝试自动修复 npm 全局 shim…");
            found = repair_pi_web_cli(ui);
        }

        if !found {
            let hint = install_hint(name);
            let msg = format!("未找到命令 `{}`，请先执行：\n   {}", name, hint);
            ui.step_fail(&format!("未找到命令 `{}`", name));
            ui.message(&format!("    需先执行：{}", hint));
            if *name == "pi-web" {
                ui.message("    自动修复失败（npm 建命令入口失败 / 包本体不在），--force 重装可重建入口");
            }
            return Err(msg);
        }
        ui::log(&format!("  ✓ 命令 `{}` 已就绪", name));
    }

    ui.step_ok("pi / pi-web / npm 全部就绪");
    Ok(())
}

/// npm 全局前缀目录（bin shim 与 node_modules 同级）。
/// 优先问 npm 本身，拿不到时在 Windows 上回退到 `%APPDATA%\npm`。
fn global_prefix() -> Option<PathBuf> {
    if let Ok(o) = run_capture("npm", &["prefix", "-g"]) {
        let p = o.stdout.trim();
        if !p.is_empty() && Path::new(p).is_dir() {
            return Some(PathBuf::from(p));
        }
    }
    let appdata = std::env::var("APPDATA").ok()?;
    let p = Path::new(&appdata).join("npm");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// npm 在 Windows 上替换 bin shim 时，先把旧 shim 改名成隐藏的临时名
/// （`.pi-web-XXXX` / `.pi-web.cmd-XXXX` / `.pi-web.ps1-XXXX`）再写新的。
/// 升级中途被杀，正式名就永久缺失、只剩这些残留。返回应复位成的正式文件名。
fn staged_shim_target(file_name: &str) -> Option<&'static str> {
    let rest = file_name.strip_prefix(".pi-web")?;
    // npm 的随机后缀由小写字母 + 数字组成，借此排除其他 `.pi-web*` 干扰文件
    let is_token = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    if let Some(t) = rest.strip_prefix(".cmd-") {
        if is_token(t) {
            return Some("pi-web.cmd");
        }
    }
    if let Some(t) = rest.strip_prefix(".ps1-") {
        if is_token(t) {
            return Some("pi-web.ps1");
        }
    }
    if let Some(t) = rest.strip_prefix('-') {
        if is_token(t) {
            return Some("pi-web");
        }
    }
    None
}

/// 补写最小可用的 `pi-web.cmd` shim（npm 的 shim 丢失但包本体还在时使用）。
fn write_cmd_shim(path: &Path, entry_rel: &str) -> std::io::Result<()> {
    let entry = entry_rel.replace('/', "\\");
    let content = format!(
        "@ECHO off\r\nsetlocal\r\nset \"dp0=%~dp0\"\r\n\
         if exist \"%dp0%node.exe\" (set \"_prog=%dp0%node.exe\") else (set \"_prog=node\")\r\n\
         \"%_prog%\" \"%dp0%{}\" %*\r\n",
        entry
    );
    std::fs::write(path, content)
}

/// 把 npm 残留的 `.pi-web*` 备份改回正式 shim 名，返回实际执行的 `(旧名, 新名)` 列表。
/// 已存在的正式文件不覆盖（避免抢走能用的旧 shim）。
fn restore_staged_shims(prefix: &Path) -> Vec<(String, &'static str)> {
    let mut done = Vec::new();
    let Ok(entries) = std::fs::read_dir(prefix) else {
        return done;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(target) = staged_shim_target(&name) else {
            continue;
        };
        let dst = prefix.join(target);
        if !dst.exists() && std::fs::rename(entry.path(), &dst).is_ok() {
            done.push((name, target));
        }
    }
    done
}

/// 尝试修复失联的 `pi-web` 命令，返回修复后是否可用：
/// 1. 把 npm 升级中断残留的 `.pi-web*` 备份复位成正式 shim 名
/// 2. 包本体在但 shim 仍缺 → 直接补写 `pi-web.cmd`
/// 3. 包本体缺失 → `npm install -g @agegr/pi-web@latest` 重装（npm 顺带生成 shim）
fn repair_pi_web_cli(ui: &mut Ui) -> bool {
    if command_exists("pi-web") {
        return true;
    }

    let Some(prefix) = global_prefix() else {
        ui::log("  ✗ 无法定位 npm 全局前缀，跳过自动修复");
        return false;
    };

    // 1) 复位残留 shim
    for (from, to) in restore_staged_shims(&prefix) {
        ui.message(&format!("  ↻ 复位 npm 残留 shim：{} → {}", from, to));
    }
    if command_exists("pi-web") {
        ui.message("  ✓ 已复位 `pi-web` shim，命令恢复可用");
        return true;
    }

    // 2) 包本体在，只是 shim 没写出来
    if prefix.join(PI_WEB_ENTRY).is_file() {
        let shim = prefix.join("pi-web.cmd");
        if write_cmd_shim(&shim, PI_WEB_ENTRY).is_ok() && command_exists("pi-web") {
            ui.message(&format!("  ✓ 已补写 shim：{}", shim.display()));
            return true;
        }
    }

    // 3) 包本体缺失：重装（npm 会同时重建 shim）
    ui.message("  ↻ 包本体缺失，执行 npm install -g @agegr/pi-web@latest 重装…");
    let _ = run_checked("npm", &["install", "-g", PI_WEB_PKG_LATEST]);
    command_exists("pi-web")
}

/// 步骤 2：执行 pi update --all
pub fn step_pi_update(ui: &mut Ui) -> Result<(), String> {
    ui.step_start(2, "执行 pi update --all", "pi update --all", false);
    ui.step_message("正在更新 Pi 及扩展包…");

    let result = run_checked("pi", &["update", "--all"]);

    match result {
        RunResult::Success => {
            ui.step_ok("Pi 及所有扩展包更新完成");
            Ok(())
        }
        RunResult::NotFound(prog) => {
            let msg = format!("{} 命令未找到，请检查安装", prog);
            ui.step_fail(&msg);
            Err(msg)
        }
        RunResult::Failure { code, stderr } => {
            ui.step_fail(&format!("pi update 失败（退出码 {}）", code));
            ui.print_stderr_summary(&stderr);
            Err("pi update 执行失败，启动中止".to_string())
        }
        RunResult::IoError(e) => {
            let msg = format!("执行 pi update 时发生 IO 错误: {}", e);
            ui.step_fail(&msg);
            Err(msg)
        }
    }
}

/// 步骤 3：检查 pi-web 是否有新版本，有则升级（npm install -g @agegr/pi-web@latest）。
///
/// 流程：
/// 0. pi-web 正在运行 → 跳过升级（进程锁定文件，npm -g 替换会 EBUSY 失败）
/// 1. `npm view @agegr/pi-web version` 查询 registry 最新版
/// 2. `npm ls -g @agegr/pi-web` 查询本地已装版本
/// 3. 本地较旧（或本地未装 / 本地查询失败）→ 执行升级安装命令
///
/// 任何查询失败都视为“无法确认”，走安全路径（直接执行安装命令由 npm 自行判断）。
pub fn step_pi_web_upgrade(ui: &mut Ui, host: &str, port: u16) -> Result<(), String> {
    ui.step_start(3, "检查并升级 pi-web", "检查并升级 pi-web", false);

    // 运行中的实例会锁定包目录，npm -g 升级必然失败（EBUSY），
    // 因此先探测：确认是 pi-web 在跑就跳过升级，保留现有版本。
    // 只认 pi-web 本体（probe_pi_web），端口被别的 HTTP 服务占着不算「已在运行」。
    if probe_pi_web(host, port) == WebProbe::Up {
        ui.step_ok(&format!(
            "pi-web 正在运行（http://{}:{}），跳过升级",
            host, port
        ));
        ui.message(
            "  ⚠ 运行中的 pi-web 会锁定文件，npm -g 升级无法替换；如需升级：先执行 `pi-web stop`，再运行 launcher。",
        );
        return Ok(());
    }

    ui.step_message("查询 registry 最新版本…");
    let remote = run_capture("npm", &["view", PI_WEB_PKG, "version"]);

    if let Err(RunResult::NotFound(prog)) = &remote {
        let msg = format!("{} 命令未找到，请检查安装", prog);
        ui.step_fail(&msg);
        return Err(msg);
    }

    let remote_ver = remote
        .ok()
        .map(|o| o.stdout)
        .as_deref()
        .and_then(extract_version);
    ui.step_message("查询本地已安装版本…");
    let local = run_capture("npm", &["ls", "-g", PI_WEB_PKG]);
    let local_ok = local.is_ok();
    let local_ver = local.ok().and_then(|o| {
        if o.success {
            extract_version(&o.stdout)
        } else {
            // npm ls 反查不存在的包时非零退出，视为「本地未装由 npm 处理」
            None
        }
    });

    match (remote_ver.as_deref(), local_ver.as_deref()) {
        (Some(r), Some(l)) if compare_versions(l, r) != std::cmp::Ordering::Less => {
            ui.step_ok(&format!("pi-web 已是最新版本（v{}）", l));
            Ok(())
        }
        (Some(r), Some(l)) => {
            ui.message(&format!("  ⚠ 发现新版本：v{} → v{}，正在升级…", l, r));
            run_upgrade_install(ui)
        }
        // 本地版本查不到 / 未安装：无法确认，直接跑安装命令让 npm 判断
        (Some(_), None) => {
            if local_ok {
                ui.message("  ⚠ 本地已安装但无法解析版本号，直接执行升级命令…");
            } else {
                ui.message("  ⚠ pi-web 未以全局包形式安装，直接执行安装…");
            }
            run_upgrade_install(ui)
        }
        // registry 查询失败：网络问题或包不存在，同样兜底直接安装
        (None, _) => {
            ui.message("  ⚠ 无法查询 registry 上的最新版本，回退为直接执行升级命令…");
            run_upgrade_install(ui)
        }
    }
}

/// 执行 `npm install -g @agegr/pi-web@latest` 并处理结果。
fn run_upgrade_install(ui: &mut Ui) -> Result<(), String> {
    ui.step_message(&format!("npm install -g {} …", PI_WEB_PKG_LATEST));

    let result = run_checked("npm", &["install", "-g", PI_WEB_PKG_LATEST]);

    match result {
        RunResult::Success => {
            // 复查一次实际安装到的版本，确认结果
            let now = run_capture("npm", &["ls", "-g", PI_WEB_PKG])
                .ok()
                .filter(|o| o.success)
                .and_then(|o| extract_version(&o.stdout))
                .unwrap_or_default();
            if now.is_empty() {
                ui.step_ok("pi-web 升级完成");
            } else {
                ui.step_ok(&format!("pi-web 升级完成，当前版本 v{}", now));
            }
            // 升级后校验 CLI 是否仍可调用：npm 被杀在 bin 链接阶段时包在、shim 没了
            if !command_exists("pi-web") && !repair_pi_web_cli(ui) {
                let msg = format!(
                    "pi-web 已安装但 CLI 不可用，请执行：{}",
                    install_hint("pi-web")
                );
                ui.step_fail(&msg);
                return Err(msg);
            }
            Ok(())
        }
        RunResult::NotFound(prog) => {
            let msg = format!("{} 命令未找到，请检查安装", prog);
            ui.step_fail(&msg);
            Err(msg)
        }
        RunResult::Failure { code, stderr } => {
            // pi-web 常驻实例会锁定包目录，npm -g 无法替换（EBUSY / busy or locked）。
            // 这类失败不中止启动：按现有版本继续，提示先 stop 后再跑一次 launcher 完成升级。
            let locked =
                code == -4082 || stderr.contains("EBUSY") || stderr.contains("busy or locked");
            if locked {
                ui.step_ok("pi-web 升级被运行中的实例文件锁挡下，按现有版本继续启动");
                ui.message("  ⚠ 如需完成升级：先执行 `pi-web stop`，再运行一次 launcher。");
                ui.message("    （npm 错误详情已记录到日志）");
                return Ok(());
            }
            // Ctrl+C / 关窗打断升级：npm 可能已把 shim 改名成备份却没写回正式名，
            // 此时包在但命令消失，先尝试自动修复再决定是否中止。
            if code == CTRL_C_EXIT_CODE || code == 130 {
                ui.message("  ⚠ pi-web 升级被中断（Ctrl+C / 窗口关闭），尝试恢复…");
                if repair_pi_web_cli(ui) {
                    ui.step_ok("升级中断，已恢复 `pi-web` 命令，按现有版本继续启动");
                    return Ok(());
                }
                let msg = format!(
                    "pi-web 升级中断且自动恢复失败，请执行：{}",
                    install_hint("pi-web")
                );
                ui.step_fail(&msg);
                return Err(msg);
            }
            ui.step_fail(&format!("pi-web 升级失败（退出码 {}）", code));
            ui.print_stderr_summary(&stderr);
            Err("pi-web 升级失败，启动中止".to_string())
        }
        RunResult::IoError(e) => {
            let msg = format!("执行 pi-web 升级时发生 IO 错误: {}", e);
            ui.step_fail(&msg);
            Err(msg)
        }
    }
}

/// 步骤 4：启动 pi-web（常驻后台服务，脱离本进程）。
///
/// - 启动前先 HTTP 探测：服务已在运行则跳过（保留原进程，本进程无从监控）
/// - 否则 detached 启动，并以确定进度条展示「等待 HTTP 就绪」的时间轴（60s 上限）
pub fn step_pi_web(ui: &mut Ui, host: &str, port: u16) -> Result<WebStart, String> {
    ui.step_start(4, "启动 pi-web（常驻后台）", "启动 pi-web（后台）", true);

    // 已在运行？直接跳过，避免端口冲突。
    // 这里必须确认「跑的是 pi-web」：端口被别的服务占着时不能当成已就绪，
    // 否则后面的 Pi CLI 会连到一个完全不相干的服务上。
    match probe_pi_web(host, port) {
        WebProbe::Up => {
            let msg = format!(
                "pi-web 已在运行：http://{}:{}，跳过启动（保留原进程，本次不做崩溃监控）",
                host, port
            );
            ui.step_ok(&msg);
            return Ok(WebStart::AlreadyRunning);
        }
        WebProbe::Foreign => {
            let msg = format!(
                "端口 {} 被非 pi-web 的 HTTP 服务占用，无法启动 pi-web；请换端口（PORT 环境变量）或停掉该服务",
                port
            );
            ui.step_fail(&msg);
            return Err(msg);
        }
        WebProbe::Down => {}
    }

    ui.step_message("正在启动 pi-web 服务…");

    let mut child = match spawn_detached("pi-web", &[]) {
        Ok(c) => c,
        Err(e) => {
            ui.step_fail(&format!("pi-web 启动失败：{}", e));
            return Err(e);
        }
    };

    ui.step_message(&format!(
        "等待 HTTP 就绪 http://{}:{}（上限 {}s）…",
        host, port, READY_TIMEOUT_SECS
    ));

    let progress = |frac: f64| {
        ui.step_progress(frac);
    };

    match wait_ready(&mut child, host, port, READY_TIMEOUT_SECS, Some(progress)) {
        StartupOutcome::Up => {
            ui.step_ok(&format!("pi-web 已启动并就绪：http://{}:{}", host, port));
            ui.message("  ⚠ pi-web 已脱离本进程，launcher / CLI 退出后仍会继续运行");
            ui.message("  ⚠ 如需停止，请执行：pi-web stop");
            Ok(WebStart::Spawned(child))
        }
        StartupOutcome::Exited => {
            ui.step_fail("pi-web 进程在就绪前就退出了，可能端口被占用或配置有误");
            Err("pi-web 启动失败".to_string())
        }
        StartupOutcome::Timeout => {
            terminate_background(&mut child);
            ui.step_fail(&format!("等待 pi-web 就绪超时（{}s）", READY_TIMEOUT_SECS));
            Err("pi-web 启动超时".to_string())
        }
    }
}

/// 步骤 5：启动 pi cli（前台交互）——仅正常模式调用，安静模式跳过。
///
/// 注意：`pi` 的 `--mode` 只接受 text / json / rpc，并没有 interactive；
/// 交互模式是在「stdin/stdout 都是 TTY 且非 --print」时的预设行为，所以直接跑 `pi`。
///
/// 启动前先复核 pi-web 是否真的就绪：步骤 4 可能只是复用了外部实例（AlreadyRunning），
/// 该实例在本进程等待期间可能已经退出；此时仍然拉起 CLI 只会让用户对着一个连不上的服务操作。
pub fn step_pi_cli(ui: &mut Ui, host: &str, port: u16) -> Result<(), String> {
    ui.step_start(5, "启动 Pi CLI 交互界面", "Pi CLI", false);

    // CLI 是 TUI：先撤掉全部活体渲染并把账目行重放为纯文本——
    // pi 的 inline TUI 与任何还在重绘的 bar 会互相糊画面（v1.1.0 回归事故）
    ui.replay_ledger();

    if probe_pi_web(host, port) != WebProbe::Up {
        let msg = format!(
            "pi-web 未就绪（http://{}:{}），已中止启动 Pi CLI",
            host, port
        );
        ui.step_fail(&msg);
        return Err(msg);
    }

    ui.message("  正在启动 Pi CLI 交互界面…");
    ui.message("  （退出 CLI 不影响已脱离运行的 pi-web）");

    let status = build_command("pi")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| {
            let msg = format!("启动 pi CLI 失败: {}", e);
            ui.step_fail(&msg);
            msg
        })?;

    if status.success() {
        ui.step_ok("Pi CLI 已正常退出");
        Ok(())
    } else {
        let code = status.code().unwrap_or(-1);
        let msg = format!("Pi CLI 异常退出（退出码 {}）", code);
        ui.step_fail(&msg);
        Err(msg)
    }
}

/// 比较两个 semver 版本号（如 "1.2.3"），相等返回 Ordering::Equal，
/// 逐段按数值比较；无法解析的部分按数值 0 处理。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let parse_num = |part: &str| -> u64 { part.parse().unwrap_or(0) };
    let mut pa = a.split('.');
    let mut pb = b.split('.');

    loop {
        match (pa.next(), pb.next()) {
            (None, None) => return Ordering::Equal,
            (x, y) => {
                let nx = x.map(parse_num).unwrap_or(0);
                let ny = y.map(parse_num).unwrap_or(0);
                match nx.cmp(&ny) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
    }
}

/// 从字符串中提取形如 x.y.z 的版本号。
/// 兼容 `npm view` 的纯版本输出与 `npm ls -g` 的 `├── pkg@1.2.3` 树状输出。
fn extract_version(text: &str) -> Option<String> {
    for raw_line in text.lines() {
        let line = raw_line.trim();

        // 1) `pkg@x.y.z` 树状输出：取包名后最后一个 '@' 之后的内容
        if let Some(i) = line.find(PI_WEB_PKG) {
            if let Some(off) = line[i..].rfind('@') {
                let candidate = line[i + off + 1..].trim();
                if is_version(candidate) {
                    return Some(candidate.to_string());
                }
            }
            continue;
        }

        // 2) `npm view ... version` 的裸版本号输出（如 `0.4.2`）
        if is_version(line) {
            return Some(line.to_string());
        }
    }
    None
}

/// 判断是否为合法版本号片段（如 `0.4.2`、`1.0.0-rc.1`、`2.0.0+build`）。
fn is_version(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_from_npm_view() {
        // `npm view @agegr/pi-web version` 的裸版本号输出
        assert_eq!(extract_version("0.4.2"), Some("0.4.2".to_string()));
        assert_eq!(extract_version("0.4.2\n"), Some("0.4.2".to_string()));
        assert_eq!(
            extract_version("1.0.0-rc.1\n"),
            Some("1.0.0-rc.1".to_string())
        );
    }

    #[test]
    fn extract_version_from_npm_ls_tree() {
        let text = "C:\\Users\\demo\\AppData\\Roaming\\npm\n└── @agegr/pi-web@1.2.3\n";
        assert_eq!(extract_version(text), Some("1.2.3".to_string()));
    }

    #[test]
    fn extract_version_rejects_garbage() {
        // npm ls 未安装该包时的输出
        let text = "C:\\Users\\demo\\AppData\\Roaming\\npm\n└── (empty)\n";
        assert_eq!(extract_version(text), None);
        assert_eq!(extract_version(""), None);
    }

    #[test]
    fn staged_shim_targets() {
        assert_eq!(staged_shim_target(".pi-web-TGMIF9N3"), Some("pi-web"));
        assert_eq!(
            staged_shim_target(".pi-web.cmd-XCdrzKM8"),
            Some("pi-web.cmd")
        );
        assert_eq!(
            staged_shim_target(".pi-web.ps1-AsxPA9Sh"),
            Some("pi-web.ps1")
        );
        // 正式文件 / 无关文件都不应被误判
        assert_eq!(staged_shim_target("pi-web.cmd"), None);
        assert_eq!(staged_shim_target(".pi-web"), None);
        assert_eq!(staged_shim_target(".pi-web-other.old"), None);
        assert_eq!(staged_shim_target("pi.cmd"), None);
    }

    #[test]
    fn restore_staged_shims_renames_backups() {
        let dir = std::env::temp_dir().join(format!("pi-launcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            ".pi-web-TGMIF9N3",
            ".pi-web.cmd-XCdrzKM8",
            ".pi-web.ps1-AsxPA9Sh",
        ] {
            std::fs::write(dir.join(name), "shim").unwrap();
        }

        let restored = restore_staged_shims(&dir);
        assert_eq!(restored.len(), 3);
        for target in ["pi-web", "pi-web.cmd", "pi-web.ps1"] {
            assert!(dir.join(target).is_file(), "{} 未被复位", target);
        }
        assert!(!dir.join(".pi-web-TGMIF9N3").exists());

        // 正式 shim 已存在时不得覆盖
        std::fs::write(dir.join(".pi-web-abcd1234"), "stale").unwrap();
        std::fs::write(dir.join("pi-web"), "current").unwrap();
        assert!(restore_staged_shims(&dir).is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("pi-web")).unwrap(),
            "current"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compare_versions_basic() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3", "1.2.4"), Ordering::Less);
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Ordering::Greater);
        // 段数不等时缺省按 0
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
    }
}
