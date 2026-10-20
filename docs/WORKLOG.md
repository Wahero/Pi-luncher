# 工作记录（WORKLOG）

按时间倒序记录已完成的实际工作：动因、证据、改动、验证与遗留项。
面向「下次接手的人或 agent」：能照着复现、能判断当时为什么这么做。

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
