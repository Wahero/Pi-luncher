# pi-launcher

Pi Agent 批量自动化启动工具（Rust 实现）。

> 仓库：<https://github.com/Wahero/Pi-luncher>
> 设计文档：[docs/Pi-Luncher.html](docs/Pi-Luncher.html)（浏览器打开，含完整设计说明与源码）

一条命令完成 **环境预检 → `pi update --all` → 检查并升级 `pi-web` → 启动 `pi-web` → 启动 Pi CLI** 的完整流程，每个环节都带状态显示与错误检查。

其中第 3 步会查询 npm registry 上 `@agegr/pi-web` 的最新版本，并与本地已安装版本比较：发现新版本（或本地查询失败 / 未安装）时自动执行 `npm install -g @agegr/pi-web@latest` 升级；已是最新则跳过。若 `pi-web` **正在运行**（实例锁定包文件，npm 无法替换），升级会被安全跳过并提示先 `pi-web stop` 后重试。

`pi-web` 以 **detached（脱离）方式**启动：launcher 会等待 HTTP 服务真正就绪后才继续，并附带一个守护线程（进程死亡自动重启，最多 3 次）。若 `pi-web` 已在运行，则跳过启动，不重复拉起、也不做崩溃监控（该实例无从追踪）。Pi CLI 退出、launcher 关闭后，`pi-web` **继续在后台运行**，需手动 `pi-web stop` 停止。

## 运行模式

| 用法 | 行为 |
|---|---|
| `pi-launcher` | 正常模式：动画进度 + Pi CLI 交互 + 结束停留 |
| `pi-launcher --display <A\|B\|C>` | 显示模式：**A=工作账·一步一行（默认）**；B=轻修·分栏仪表；C=极简·一行流（`a/b/c`、`ledger/lite/min` 别名均可）|
| `pi-launcher --quiet` | 安静模式（`-q` / `--auto` 别名）：几乎零输出、**跳过 Pi CLI**，完成或失败都自动关窗；细节全在日志 |
| `pi-launcher --spin <名字>` | 指定右侧动画（`dots` / `dots2` / `grow` / `moon` / `clock` / `hourglass`）|
| `pi-launcher -h` / `--help` | 参数说明 |
| `pi-launcher --force-tty` | 调试用：非终端环境强制走动画渲染路径 |

- **结束停留**：正常模式结束后按 Enter 立即关闭；**30 秒无输入自动倒计时关闭**（成功、失败路径一致）。非交互终端（管道 / CI）直接关闭。
- **显示模式（仅交互终端生效）**：`--display` 每档的开场与行式不同；quiet 与非终端自动退化为纯文字，与该参数无关。非法值（如 `--display x`）打印可选项后以退出码 1 结束。
- **动画轮换**：右侧动画从 6 组池中按本机启动次数循环选用（每次启动自动换一个），也可用 `--spin` 固定。
- **运行日志**：两种模式都会写 `%LOCALAPPDATA%\pi-launcher\logs\pi-launcher-<时间戳>.log`（UTF-8 带 BOM；含时间戳、步骤开始 / 完成 / 失败、升级版本对比、错误摘要、最终退出码），每次启动新建文件，自动保留最近 20 份。

## 前置依赖

- Rust 2021 工具链（`rustc` / `cargo`）
- 已安装 Pi Coding Agent：

  ```bash
  npm install -g --ignore-scripts @earendil-works/pi-coding-agent
  ```

  需确保 `pi`、`pi-web` 两个命令在 `PATH` 中。

## 编译与运行

```bash
# 开发版
cargo build
# 可执行文件：target/debug/pi-launcher（Windows 为 pi-launcher.exe）

# 发布版
cargo build --release
# 可执行文件：target/release/pi-launcher

# 安装到 ~/.cargo/bin，全局可用
cargo install --path .

# 直接编译并运行
cargo run --release
```

## 运行效果

正常模式默认为 **A · 工作账**（`--display a`）——一步一行、完成瞬间原位凝固：

```
  PI LAUNCHER · v1.1.0                        动画 dots · 池 1/6 组
  ──────────────────────────────────────────────────────────────────────

  1/5 环境预检                ⠙ 0.3s 检查命令 `npm` …
  2/5 pi update --all         ✓ 5.6s Pi 及所有扩展包更新完成
  3/5 检查并升级 pi-web       ✓ 1.2s 已是最新（3.4.2）
  4/5 启动 pi-web（后台）      12.4s 等待就绪 ━━━╸────────── 62% eta:22
  5/5 Pi CLI                  ⠋ 8.1s 交互中 …

  整体进度 ━━━━━━━━━━╸──────────────── 80%
```

收尾时整体条撤场，账目行保持为普通行，随后是日志路径与「✓ 全部步骤完成」；每一行的右侧动作（转圈 / 进度条 + eta、当前操作名）都在固定位置实时刷新。等待 `pi-web` HTTP 就绪时展示真实比例进度条（60 秒时间轴）。

另两档样式（`--display b` / `--display c`）：**B · 轻修** 保留分栏仪表节奏、开场为圆角细框；**C · 极简** 无框无分隔线的一行短流。quiet 与非终端自动退化为纯文字样式，不受该参数影响。

停止后台服务：

```bash
pi-web stop
```

## 退出码约定

| 退出码 | 含义 |
|:---:|---|
| `0` | 全部成功 |
| `1` | 环境预检失败 |
| `2` | `pi update` 失败 |
| `3` | `pi-web` 检查 / 升级失败 |
| `4` | `pi-web` 启动失败 |
| `5` | Pi CLI 异常 |

## 项目结构

```
pi-launcher/
├── Cargo.toml          # 依赖配置（含 [profile.release] 体积优化，见下方规则）
├── src/
│   ├── main.rs         # 主入口、流程编排、supervisor、结束停留
│   ├── ui.rs           # 统一输出层（交互动画 / 纯文字 / 安静）+ 文字日志
│   ├── command_runner.rs  # 命令执行、就绪探测、detached 启动
│   └── steps.rs        # 各启动步骤定义与执行
├── tools/
│   └── gen-docs.ps1    # 从当前源码再生成 docs/Pi-Luncher.html
├── docs/
│   └── Pi-Luncher.html # 完整设计说明（与源码同步，v1.1.0；
│                       #   可直接在浏览器打开，或点开仓库内文件渲染）
├── AGENTS.md           # 项目规则（体积优化等，构建代理必读）
└── README.md
```

## 减小发布版体积

**本项目重要规则**：每次生成目标码都保持程序体积优化。`Cargo.toml` 已内置以下配置（勿移除或弱化；调整 profile 后核对 `target/release/pi-launcher.exe` 体积不得明显回退，v1.0.0 约 390 KB）：

```toml
[profile.release]
opt-level = "z"
lto = true
strip = true
codegen-units = 1
panic = "abort"
```

v1.1.2 实测 408,064 字节（≈398.5 KB；较 v1.0.0 基线 +2.2%，为新增 `--display` 三档显示引擎；v1.1.1 的 TUI 交接修复与 v1.1.2 的结束停留修复均保持同尺寸）。

## 常见问题

- **依赖下载慢**：配置国内镜像（`~/.cargo/config.toml`），例如 USTC 源。
- **报错找不到模块**：确认 `src/main.rs` 中有 `mod command_runner; mod steps;`，且对应文件存在、大小写正确。
- **运行时报 `pi` / `pi-web` 找不到**：这是运行环境问题而非编译问题，需先执行上面的 `npm install -g` 安装命令。

## 许可

[Apache-2.0](LICENSE)
