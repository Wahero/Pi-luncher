use crate::command_runner::{
    build_command, command_exists, probe_http, run_capture, run_checked, spawn_detached,
    terminate_background, wait_ready, RunResult, StartupOutcome,
};
use crate::ui::{self, Ui};
use std::process::Child;

/// 启动步骤总数（与 main 的流程一一对应）
pub const TOTAL_STEPS: usize = 5;

/// pi-web 就绪探测的等待上限（秒）。首次启动可能要下载/编译，给足时间。
const READY_TIMEOUT_SECS: u64 = 60;

/// pi-web 服务包名（npm 发起升级时使用）
const PI_WEB_PKG: &str = "@agegr/pi-web";
/// 升级命令的版本标签（始终安装 registry 上的最新版）
const PI_WEB_PKG_LATEST: &str = "@agegr/pi-web@latest";

/// 步骤 4 的结果：本次 spawn 的 pi-web 交给 supervisor 监控；
/// 已在运行的（此前脱离启动的实例）无从监控，跳过即可。
#[derive(Debug)]
pub enum WebStart {
    Spawned(Child),
    AlreadyRunning,
}

/// 步骤 1：环境预检（pi / pi-web / npm）
/// 行内只显示一行（右上侧动态报当前检查项），逐项就绪写进日志。
pub fn step_preflight(ui: &mut Ui) -> Result<(), String> {
    ui.step_start(1, "环境预检（pi / pi-web / npm）", "环境预检", false);

    let required = ["pi", "pi-web", "npm"];
    for name in &required {
        ui.step_message(&format!("检查命令 `{}` …", name));
        if !command_exists(name) {
            let msg = format!(
                "未找到命令 `{}`。请先安装 Pi Coding Agent：\n   npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
                name
            );
            ui.step_fail(&format!("未找到命令 `{}`", name));
            ui.message(
                "    需先执行：npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
            );
            return Err(msg);
        }
        ui::log(&format!("  ✓ 命令 `{}` 已就绪", name));
    }

    ui.step_ok("pi / pi-web / npm 全部就绪");
    Ok(())
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
    // 因此先探测：在运行就跳过升级，保留现有版本。
    if probe_http(host, port) {
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

    // 已在运行？直接跳过，避免端口冲突
    if probe_http(host, port) {
        let msg = format!(
            "pi-web 已在运行：http://{}:{}，跳过启动（保留原进程，本次不做崩溃监控）",
            host, port
        );
        ui.step_ok(&msg);
        return Ok(WebStart::AlreadyRunning);
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
pub fn step_pi_cli(ui: &mut Ui) -> Result<(), String> {
    ui.step_start(5, "启动 Pi CLI 交互界面", "Pi CLI", false);

    // CLI 是 TUI：先撤掉全部活体渲染并把账目行重放为纯文本——
    // pi 的 inline TUI 与任何还在重绘的 bar 会互相糊画面（v1.1.0 回归事故）
    ui.replay_ledger();

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
