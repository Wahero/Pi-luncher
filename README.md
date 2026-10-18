# pi-launcher

Pi Agent 批量自动化启动工具（Rust 实现）。

一条命令完成 **环境预检 → `pi update --all` → 检查并升级 `pi-web` → 启动 `pi-web` → 启动 Pi CLI** 的完整流程，每个环节都带状态显示与错误检查。

其中第 3 步会查询 npm registry 上 `@agegr/pi-web` 的最新版本，并与本地已安装版本比较：发现新版本（或本地查询失败 / 未安装）时自动执行 `npm install -g @agegr/pi-web@latest` 升级；已是最新则跳过。若 `pi-web` **正在运行**（实例锁定包文件，npm 无法替换），升级会被安全跳过并提示先 `pi-web stop` 后重试。

`pi-web` 以 **detached（脱离）方式**启动：launcher 会等待 HTTP 服务真正就绪后才继续，并附带一个守护线程（进程死亡自动重启，最多 3 次）。若 `pi-web` 已在运行，则跳过启动，不重复拉起、也不做崩溃监控（该实例无从追踪）。Pi CLI 退出、launcher 关闭后，`pi-web` **继续在后台运行**，需手动 `pi-web stop` 停止。

## 运行模式

| 用法 | 行为 |
|---|---|
| `pi-launcher` | 正常模式：动画进度 + Pi CLI 交互 + 结束停留 |
| `pi-launcher --quiet` | 安静模式（`-q` / `--auto` 别名）：几乎零输出、**跳过 Pi CLI**，完成或失败都自动关窗；细节全在日志 |
| `pi-launcher --spin <名字>` | 指定右侧动画（`dots` / `dots2` / `grow` / `moon` / `clock` / `hourglass`）|
| `pi-launcher -h` / `--help` | 参数说明 |

- **结束停留**：正常模式结束后按 Enter 立即关闭；**30 秒无输入自动倒计时关闭**（成功、失败路径一致）。非交互终端（管道 / CI）直接关闭。
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

```
  ╔══════════════════════════════════════════╗
  ║  Pi Agent 批量自动化启动工具 v1.0.0      ║
  ╚══════════════════════════════════════════╝

  将依次执行：
    1. 环境预检
    2. pi update --all
    3. 检查并升级 pi-web
    4. 启动 pi-web（后台）
    5. 启动 Pi CLI（前台交互）

  [1/5] 环境预检（pi / pi-web / npm）
  ✓ 命令 `pi` 已就绪
  ✓ 命令 `pi-web` 已就绪
  ...
  ✓ pi / pi-web / npm 全部就绪（0.2s）
```

交互终端下每个步骤行右侧有固定位置的动画 / 进度条（含已耗时），底部常驻「整体进度」；等待 `pi-web` HTTP 就绪时展示真实比例进度条（60 秒时间轴）。非交互终端自动退化为上面的纯文字样式。

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
├── Cargo.toml          # 依赖配置
├── src/
│   ├── main.rs         # 主入口、流程编排、supervisor、结束停留
│   ├── ui.rs           # 统一输出层（交互动画 / 纯文字 / 安静）+ 文字日志
│   ├── command_runner.rs  # 命令执行、就绪探测、 detached 启动
│   └── steps.rs        # 各启动步骤定义与执行
├── docs/
│   └── Pi-Luncher.html # 完整设计说明（与源码同步，v1.0.0）
└── README.md
```

## 减小发布版体积

在 `Cargo.toml` 末尾加入：

```toml
[profile.release]
opt-level = "z"
lto = true
strip = true
codegen-units = 1
panic = "abort"
```

## 常见问题

- **依赖下载慢**：配置国内镜像（`~/.cargo/config.toml`），例如 USTC 源。
- **报错找不到模块**：确认 `src/main.rs` 中有 `mod command_runner; mod steps;`，且对应文件存在、大小写正确。
- **运行时报 `pi` / `pi-web` 找不到**：这是运行环境问题而非编译问题，需先执行上面的 `npm install -g` 安装命令。
