# 工作记录（WORKLOG）

按时间倒序记录已完成的实际工作：动因、证据、改动、验证与遗留项。
面向「下次接手的人或 agent」：能照着复现、能判断当时为什么这么做。

---

## 2026-10-21 · v1.1.4 动画不显示修复 + 两个依赖缺口

### 1. 起点：三类现象

| 现象 | 影响 |
|---|---|
| 正常模式「判定是终端」，屏幕上却没有任何动画帧 | 用户以为卡住，实际步骤在跑 |
| `--force-tty` 明明加了，仍然一个帧都没有 | 调试手段失效，无法在 CI / 重定向下抓帧 |
| 端口被别的 HTTP 服务占着时，launcher 当成「pi-web 已在运行」并对着它启动 CLI | 用户对着不相干的服务操作，且不给任何提示 |

### 2. 动画不显示的四个根因

| # | 根因 | 修复 |
|---|---|---|
| 1 | `MultiProgress::new()` 默认画到 **stderr**（`ProgressDrawTarget::stderr()`），而「是不是终端」判定读的是 **stdout**。stderr 被重定向时，indicatif 判定不是终端 → 整块渲染静默丢弃，连 `println` 都不出 | 改 `ProgressDrawTarget::stdout()`：判定读哪里，渲染就画哪里 |
| 2 | `--force-tty` 只绕过了 launcher 自己的 tty 判定，indicatif 内部还会再查一次 `is_term()`，非终端照样整块隐藏（所以「加了参数没反应」） | 新增 `ForcedTerm`（实现 `TermLike`，固定报告 100×30 终端、光标移动 / 清行 / 写行逐帧直写 stdout），`--force-tty` 时用 `term_like_with_hz(Box::new(ForcedTerm), 20)` 作为 draw target |
| 3 | 非交互终端下完全静默，用户无从判断是「没动画」还是「没开始」 | 纯文字模式开头打印 `ℹ 非交互终端：动画渲染已关闭（在终端窗口直接运行，或加 --force-tty 可看到动画）`，同时写日志 |
| 4 | 轮换序号靠「本机日志文件数 % 池长」，而日志只保留最近 `LOG_KEEP`（20）份 → 第 21 次启动起日志数恒为 20，永久停在同一组动画 | 改单字节计数文件 `%LOCALAPPDATA%\pi-launcher\anim.seq`（只存当前池下标，避免把数字解析 / 格式化代码拉进二进制） |

体积取向的细节：`forced_csi` 手写十进制拼 CSI，避免每帧一次 `format!`；计数文件用 `File` 单字节读写而非 `fs::read`（后者会引入整套 `read_to_end`）。

### 3. 两个依赖缺口

| # | 缺口 | 后果 |
|---|---|---|
| ① | 步骤 5 不校验 pi-web 就拉起 CLI | 步骤 4 若复用的是**外部**实例，该实例可能在这期间退出；CLI 仍会启动，用户对着一个连不上的服务操作 |
| ② | `probe_http` 只证明「端口上有 HTTP 回应」，不判断是不是 pi-web | 任何本地 Web 服务都会被当成「pi-web 已在运行」→ 步骤 3 跳过升级、步骤 4 跳过启动，问题被静默带进步骤 5 |

### 4. 修复

| 文件 | 改动 |
|---|---|
| `src/command_runner.rs` | 新增 `WebProbe { Up, Foreign, Down }` 与 `probe_pi_web()`：读响应开头 16 KB，按 `<title>Pi Web</title>`（大小写 / 标签内空白不敏感）或 `X-Powered-By: pi-web` 判定身份；新增 `fetch_response_head()`；`probe_http` 保留给「端口是否就绪」的轮询热路径（每 250 ms 一次，必须廉价） |
| `src/steps.rs` | 步骤 3：只在 `Up` 时跳过升级；步骤 4：`Up` → 跳过启动，`Foreign` → `step_fail` 并 `exit 4`（提示换 `PORT` 或停掉该服务），`Down` → 正常启动；步骤 5 增加 `host` / `port` 参数，`replay_ledger()` 之后、拉起 CLI 之前复核 `Up`，否则 `step_fail` + `exit 5` |
| `src/main.rs` | `step_pi_cli(&mut ui, &host, port)` |
| `src/ui.rs` | 四个动画根因的修复（见上）；`VERSION` → 1.1.4 |
| `README.md` | 端口身份校验、步骤 5 复核、`--force-tty` 真实语义、非交互提示、`anim.seq`、退出码 4/5 的语义补充、体积段落 |
| `AGENTS.md` | 体积基线 → 413,184 B 并写明构成 |

### 5. 验证

**单元测试**：`cargo test` → **12 passed**（原 8 + 新增 4）

| 用例 | 断言 |
|---|---|
| `probe_recognizes_pi_web` | 假服务返回 `<title>Pi Web</title>` → `Up` |
| `probe_rejects_other_http_service` | 假服务返回 `<title>Some Other App</title>` → `Foreign` |
| `probe_reports_down_when_nothing_listens` | 端口无人应答 → `Down` |
| `marker_matching_is_case_insensitive` | `<TITLE>PI WEB</TITLE>` / `X-Powered-By: pi-web` 命中；Grafana / 404 不命中 |

**端到端**（桩 `pi` / `npm`，`--force-tty`，日志取自 `%LOCALAPPDATA%\pi-launcher\logs`）：

| 场景 | 结果 |
|---|---|
| 假服务以 `<title>Grafana</title>` 占住 39998 端口 | 步骤 4 `失败：端口 39998 被非 pi-web 的 HTTP 服务占用…`，**exit 4**，步骤 5 无记录 |
| 真 pi-web 在 30141 | 步骤 3/4 正确跳过、步骤 5 放行，**exit 0**（未误伤） |
| 假服务先以 Pi Web 标题应答两次再关监听（模拟外部实例随即退出） | 步骤 3/4 跳过，步骤 5 `失败：pi-web 未就绪（http://127.0.0.1:39997），已中止启动 Pi CLI`，**exit 5** |
| 既有失败短路（预检 / update / 升级 / 启动） | exit 1/2/3/4，后续步骤无记录 |
| `--force-tty` 抓帧 | 抓到 7 个月亮帧（修复前 0 帧） |
| detached 承诺 | 起 pi-web → 强杀 launcher → `port39996 still listening: True`，pi-web 存活 |

`cargo fmt --check`：本次改动的 `command_runner.rs` / `steps.rs` 干净（`main.rs:270`、`ui.rs:523`、`ui.rs:774` 为仓库原有格式差异，未动）。

### 6. 体积

| 阶段 | `target/release/pi-launcher.exe` |
|---|---|
| v1.1.3 基线 | 408,576 B |
| + 动画修复（4 根因） | 411,136 B（+2,560 B） |
| + pi-web 身份探测 / 步骤 5 探活 | **413,184 B**（+2,048 B） |
| 合计 | **413,184 B（+4,608 B / +1.13%）** |

构建期间 `target\release` 曾被一个 19:22 启动的旧实例占用（`os error 5`），改用 `CARGO_TARGET_DIR=%TEMP%\pl-build` 构建，产物同为 413,184 B；用户关闭该窗口后在 `target\release` 重新构建，尺寸一致（说明交替目录不影响产物）。release profile 五项未动。

版本号从 1.1.3 升到 1.1.4 后再构建，产物仍为 413,184 B——v1.1.3 时的 +512 B 未复现，说明它是段对齐的偶然差异而非固定规律；`AGENTS.md` 中该条已改为「有时 +512 B，需排除」。

### 7. 发布

```
commit 5c698e2  v1.1.4：修复动画不显示，补上 pi-web 身份校验与启动前探活
tag    v1.1.4   （annotated，已推送）
main → origin/main 同步
```

改动文件：`src/ui.rs`、`src/steps.rs`、`src/command_runner.rs`、`src/main.rs`、`Cargo.toml`、`Cargo.lock`、
`README.md`、`AGENTS.md`、`tools/gen-docs.ps1`、`docs/Pi-Launcher.html`（由 `tools/gen-docs.ps1` 重新生成）。

### 8. 遗留

- 身份标识依赖 pi-web 首页标题。若将来 pi-web 改标题，会被判成 `Foreign` 并**显式报错**（而非静默误判）；届时只需改 `looks_like_pi_web()` 一处。
- 关闭控制台窗口是否会连带结束已 detach 的 pi-web，本次只验证了「父进程被杀后 pi-web 存活」，未能在无 GUI 环境下复现 CTRL_CLOSE 语义；若再遇到「关窗后 pi-web 消失」，优先查是否被 `pi-web stop` 或任务管理器一起结束。

---

## 2026-10-21 · v1.1.3 环境预检提示按命令拆分

### 1. 起点：用户报错

启动 `pi-launcher v1.1.2`，1/5 环境预检失败：

```
[1/5] 环境预检（pi / pi-web / npm）
✗ 未找到命令 `pi-web`
  需先执行：npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

### 2. 诊断：不是 launcher 的 bug，是 npm 建 shim 没做完

| 检查 | 结果 |
|---|---|
| `npm ls -g --depth=0` | `@agegr/pi-web@0.12.2` **已安装** |
| `where pi-web` | 找不到 |
| `%APPDATA%\npm` 目录 | 只有 `.pi-web-TGMIF9N3`、`.pi-web.cmd-XCdrzKM8`、`.pi-web.ps1-AsxPA9Sh`（均 2026/8/31 22:45） |

结论：`npm install -g` 在 Windows 上建命令入口是「写临时文件 `.pi-web-xxxx` → rename 成 `pi-web`」两步，
最后一步 rename 失败/被丢弃（典型原因：安装时目标被占用报 `EPERM/EBUSY`，或安装被中断、杀软锁文件），
于是包在磁盘上、命令不在 PATH 里。

同时暴露 launcher 的文案 bug：`pi` 与 `pi-web` 是两个独立 npm 包，缺 `pi-web` 却让装
`@earendil-works/pi-coding-agent`，按提示操作永远修不好。

### 3. 修复：本机环境（无需联网）

```powershell
cd $env:APPDATA\npm
Rename-Item .pi-web-TGMIF9N3      pi-web
Rename-Item .pi-web.cmd-XCdrzKM8  pi-web.cmd
Rename-Item .pi-web.ps1-AsxPA9Sh  pi-web.ps1
```

验证：`where pi-web` 命中；`pi-web --version` → `0.12.2`；`pi --version` → `1.3.0`。

备选（同样有效）：`npm install -g @agegr/pi-web@latest --force`（先 `pi-web stop` 避免文件锁）。

### 4. 改动：v1.1.3 源码

`src/steps.rs` 环境预检拆出按命令区分的安装提示：

| 缺失命令 | 提示 |
|---|---|
| `pi` | `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` |
| `pi-web` | `npm install -g @agegr/pi-web` + 「已安装却仍缺命令（npm 建命令入口失败），用 `--force` 重装即可修复」 |
| `npm` | `https://nodejs.org/` 安装 Node.js（装完重开终端） |

退出码契约不变（预检失败仍 `exit 1`）。`README.md` 前置依赖同步拆成两条安装命令并加了 shim 残留排障注记。

实测（PATH 里只放 `pi` 桩、无 `pi-web`）：

```
[1/5] 环境预检（pi / pi-web / npm）
✗ 未找到命令 `pi-web`
  需先执行：npm install -g @agegr/pi-web
  若已安装却仍缺命令（npm 建命令入口失败），用 --force 重装即可修复
exit=1
```

### 5. 版本与体积（重要经验）

| 组合 | `target/release/pi-launcher.exe` |
|---|---|
| 代码改动 + 版本 1.1.2 | 408,064 B |
| 代码改动 + 版本 1.1.3 | **408,576 B**（+512 B / +0.13%） |

单改 `Cargo.toml` 的 `version` 就会 +512 B（crate 元数据变化引起的段对齐差异），与功能代码无关。
验证方式：把 `ui.rs` 的 `VERSION` 改回 1.1.2 重建仍是 408,576 B；只有 `Cargo.toml` 也回退才回到 408,064 B。

因此基线记录改为：**v1.1.3 = 408,576 B 为新基线**（`AGENTS.md` / `README.md` 已同步说明「+0.13% 仅版本号所致」）。
release profile 五项（`opt-level="z"` / `lto` / `strip` / `codegen-units=1` / `panic="abort"`）未动。

### 6. 发布

```
commit 99d688a  v1.1.3：预检提示按命令拆分——pi-web 指向 @agegr/pi-web，补充 npm 建 shim 失败的修复提示
tag    v1.1.3   （annotated，已推送）
main → origin/main 同步
```

改动文件：`src/steps.rs`、`src/ui.rs`、`Cargo.toml`、`Cargo.lock`、`README.md`、`AGENTS.md`、
`tools/gen-docs.ps1`、`docs/Pi-Launcher.html`（由 `tools/gen-docs.ps1` 重新生成）。

### 7. 本机一次性环境搭建（本次会话新增）

机器原本没有 Rust 工具链，为核对体积基线临时搭好：

```powershell
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--quiet --wait --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"   # cargo 未写入当前会话 PATH
```

实测版本：`cargo 1.99.1`、`rustc 1.99.1`（stable-x86_64-pc-windows-msvc）、BuildTools 17.14.42。
构建耗时：首次约 31s（含拉依赖），增量约 8s。

### 8. 复用清单：`pi` / `pi-web` 命令找不到时

1. `npm ls -g --depth=0` —— 包到底装没装；
2. `where <cmd>` —— 命令入口在不在 PATH；
3. 若「装了却找不到」，看 `%APPDATA%\npm` 有无 `.<cmd>-xxxx` 临时文件 → 改名或 `--force` 重装；
4. 仍不行再看 npm 全局 bin 是否在 PATH（`npm prefix -g`）。

---

## 2026-10-20 · 仓库落地

- 从 `https://github.com/Wahero/Pi-luncher` 克隆到 `C:\AI\@Project\Pi-Luncher`（分支 `main`，HEAD `fb682ef`，标签 v1.0.0–v1.1.2 齐全）。
- 确认构建规则（`AGENTS.md`）：发布版体积优先，改动前后需核对 `target/release/pi-launcher.exe` 大小。
