# 一次性脚本：用当前源码重新生成 docs/Pi-Luncher.html（v1.1.5 同步版）。
# 旧文档的手写高亮无法逐段维护，改为：沿用旧 CSS/外壳 + 源码纯文本转义注入。
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8

# 仓库根目录 = 本脚本所在 tools\ 的上一级
$repo = Split-Path -Parent $PSScriptRoot
$doc  = Join-Path $repo 'docs\Pi-Luncher.html'

function Escape-Html([string]$s) {
    $s -replace '&', '&amp;' -replace '<', '&lt;' -replace '>', '&gt;'
}

function Code-Block([string]$path, [string]$filename, [string]$lang) {
    $src = [System.IO.File]::ReadAllText($path)
    $esc = Escape-Html $src
@"
    <div class="code-block">
      <div class="code-header">
        <span class="filename">$filename</span>
        <span class="lang">$lang</span>
      </div>
<pre><code>$esc</code></pre>
    </div>
"@
}

# ── 旧 CSS（沿用视觉外壳）───────────────────────────────────────────
$old = [System.IO.File]::ReadAllText($doc)
$i = $old.IndexOf('<style>'); $j = $old.IndexOf('</style>')
$css = $old.Substring($i, $j - $i + 8)

# ── 源码注入 ──────────────────────────────────────────────────────
$cargo  = Code-Block (Join-Path $repo 'Cargo.toml') 'Cargo.toml' 'TOML'
$runner = Code-Block (Join-Path $repo 'src\command_runner.rs') 'src/command_runner.rs' 'RUST'
$steps  = Code-Block (Join-Path $repo 'src\steps.rs') 'src/steps.rs' 'RUST'
$ui     = Code-Block (Join-Path $repo 'src\ui.rs') 'src/ui.rs' 'RUST'
$main   = Code-Block (Join-Path $repo 'src\main.rs') 'src/main.rs' 'RUST'

$html = @"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Pi Agent 批量自动化启动工具 - Rust 实现 v1.1.5</title>
$css
</head>
<body>

<div class="layout">

  <!-- 侧边栏 -->
  <aside class="sidebar">
    <h3>目录</h3>
    <a href="#intro">项目简介</a>
    <a href="#structure">项目结构</a>
    <a href="#cargo">Cargo.toml</a>
    <a href="#runner">命令执行模块</a>
    <a href="#steps">步骤执行模块</a>
    <a href="#ui">统一输出层</a>
    <a href="#main">主入口</a>
    <a href="#modes">运行模式与参数</a>
    <a href="#exitcodes">退出码与失败策略</a>
    <a href="#build">编译可执行文件</a>
    <a class="sub" href="#build-basic">编译与安装</a>
    <a class="sub" href="#build-size">减小体积</a>
    <a class="sub" href="#build-trouble">编译排错</a>
    <a href="#design">设计说明</a>
  </aside>

  <!-- 主内容 -->
  <main class="main">

    <header id="intro">
      <span class="badge">RUST · CLI 工具 · 正式版</span>
      <h1>Pi Agent 批量自动化启动工具</h1>
      <p>
        一个用 Rust 编写的启动器工具，一条命令完成
        <strong>环境预检 → pi update --all → 检查并升级 pi-web → 启动 pi-web → 启动 Pi CLI</strong>
        的完整流程。每个环节都有状态显示与错误检查；三档显示模式（工作账 / 轻修 / 极简）随 <code>--display</code> 切换；
        支持安静模式（自动化 / 计划任务）与全程文字日志。
      </p>
      <div class="tags">
        <span>v1.1.5</span>
        <span>Rust 2021</span>
        <span>indicatif 0.17</span>
        <span>脱离式后台</span>
        <span>--quiet 安静模式</span>
        <span>--display A/B/C</span>
        <span>全程文字日志</span>
      </div>
    </header>

    <!-- 项目简介 -->
    <section>
      <h2><span class="num">1</span>项目简介</h2>

      <ul class="steps-list">
        <li><strong>环境预检</strong> — 确认 <code>pi</code> / <code>pi-web</code> / <code>npm</code> 在 PATH 中可执行</li>
        <li><strong>pi update --all</strong> — 更新 Pi 主体与所有扩展包</li>
        <li><strong>检查并升级 pi-web</strong> — <code>npm view</code> 对比 registry 最新版本，需要时 <code>npm install -g @agegr/pi-web@latest</code></li>
        <li><strong>启动 pi-web</strong> — detached 常驻后台 + HTTP 就绪探测（60 秒上限，真实比例进度条）</li>
        <li><strong>启动 Pi CLI</strong> — 前台交互界面（安静模式跳过）</li>
      </ul>

      <div class="features">
        <div class="feature-card">
          <span class="icon">🚀</span>
          <div class="name">一条命令无人值守</div>
          <div class="desc">失败自动定位（不同错误对应不同退出码），成功后进入 CLI；亦可全程免操作。</div>
        </div>
        <div class="feature-card">
          <span class="icon">🛠</span>
          <div class="name">脱离式后台服务</div>
          <div class="desc">pi-web 脱离 launcher 运行，launcher / CLI 退出后不受影响，并带守护线程自动重启。</div>
        </div>
        <div class="feature-card">
          <span class="icon">🌙</span>
          <div class="name">安静模式</div>
          <div class="desc">--quiet 几乎零输出、跳过 CLI、自动关窗；适合 Windows 计划任务与开机自启。</div>
        </div>
        <div class="feature-card">
          <span class="icon">🧾</span>
          <div class="name">全程文字日志</div>
          <div class="desc">两种模式都落盘：步骤起止、版本对比、错误摘要、最终退出码；自动保留最近 20 份。</div>
        </div>
        <div class="feature-card">
          <span class="icon">✨</span>
          <div class="name">动画进度</div>
          <div class="desc">右侧固定位置动画（6 组池轮换）+ 底部整体进度；就绪等待展示真实比例。</div>
        </div>
        <div class="feature-card">
          <span class="icon">↩</span>
          <div class="name">倒计时关窗</div>
          <div class="desc">正常模式结束按 Enter 立即关闭；30 秒无输入自动倒计时关闭（成功/失败一致，先清空 pi 残留按键）。</div>
        </div>
      </div>
    </section>

    <!-- 项目结构 -->
    <section id="structure">
      <h2><span class="num">2</span>项目结构</h2>
      <div class="file-tree">
<pre>pi-launcher/
├── Cargo.toml        # 依赖配置（indicatif / console / ctrlc / windows-sys 等）
├── src/
│   ├── main.rs       # 主入口：参数解析、流程编排、supervisor、30 秒倒计时
│   ├── ui.rs         # 统一输出层：交互动画 / 纯文字 / 安静 + 文字日志
│   ├── steps.rs      # 1-5 号启动步骤实现
│   └── command_runner.rs  # 命令执行、HTTP 探测、detached 启动
├── docs/
│   └── Pi-Luncher.html  # 本文档（与源码同步生成）
└── README.md</pre>
      </div>
      <div class="callout info">
        <span class="title">模块要点</span>
        所有控制台输出都经由 <code>src/ui.rs</code>，其余模块不直接使用 <code>println!</code>，保证三种渲染模式与日志行为一致。
      </div>
    </section>

    <!-- Cargo.toml -->
    <section id="cargo">
      <h2><span class="num">3</span>Cargo.toml</h2>
      <p>依赖保持最小化；<code>windows-sys</code> 仅用于 <code>GetLocalTime</code> 取本地时间戳（日志）。</p>
      $cargo
    </section>

    <!-- command_runner.rs -->
    <section id="runner">
      <h2><span class="num">4</span>命令执行模块</h2>
      <p>封装命令查找、执行、结果枚举与 HTTP 探测；<code>wait_ready</code> 支持进度回调，让「等待 pi-web 就绪」既有真实退出检测又有确定进度条。</p>
      $runner
    </section>

    <!-- steps.rs -->
    <section id="steps">
      <h2><span class="num">5</span>步骤执行模块</h2>
      <p>五个步骤顺序执行，全部经 <code>Ui</code> 输出与落盘：</p>
      <table>
        <thead><tr><th>步骤</th><th>内容</th><th>关键点</th></tr></thead>
        <tbody>
          <tr><td>1 预检</td><td><code>pi</code> / <code>pi-web</code> / <code>npm</code> 逐个查</td><td>缺命令直接失败（退出码 1）并提示安装命令</td></tr>
          <tr><td>2 更新</td><td><code>pi update --all</code></td><td>失败打印 stderr 摘要（退出码 2）</td></tr>
          <tr><td>3 升级检查</td><td><code>npm view</code> vs <code>npm ls -g</code></td><td>本地较旧才升级；pi-web 运行中先跳过（保护文件锁）；EBUSY 文件锁警告后按现版继续（退出码 3 仅真正的升级失败）</td></tr>
          <tr><td>4 启动 pi-web</td><td>detached spawn + HTTP 就绪探测</td><td>已在运行则跳过（保留原进程，不做监控）；提前退出 / 超时分别处理（退出码 4）</td></tr>
          <tr><td>5 Pi CLI</td><td>前台 <code>pi</code> 交互</td><td>启动前清掉进度条避免 TUI 重绘冲突（退出码 5）</td></tr>
        </tbody>
      </table>
      $steps
    </section>

    <!-- ui.rs -->
    <section id="ui">
      <h2><span class="num">6</span>统一输出层</h2>
      <p>三种渲染共用一个类型接口；跨线程输出（supervisor）走 <code>notify()</code>。</p>
      <div class="features">
        <div class="feature-card">
          <span class="icon">🎬</span>
          <div class="name">Interactive</div>
          <div class="desc">MultiProgress：步骤行右侧固定动画/进度 + 底部整体进度；普通行挂在 MultiProgress 上安全打印。</div>
        </div>
        <div class="feature-card">
          <span class="icon">📊</span>
          <div class="name">显示模式</div>
          <div class="desc">--display A（工作账·默认）一步一行、完成即原位凝固；B（轻修）分栏仪表；C（极简）一行流。</div>
        </div>
        <div class="feature-card">
          <span class="icon">📄</span>
          <div class="name">Text</div>
          <div class="desc">非 TTY（管道 / CI）：纯文字输出，无渲染开销，结果行带耗时。</div>
        </div>
        <div class="feature-card">
          <span class="icon">🤫</span>
          <div class="name">Quiet</div>
          <div class="desc">--quiet：零输出；所有 message 自动写进日志。</div>
        </div>
      </div>
      <div class="callout note">
        <span class="title">动画池轮换</span>
        6 组（dots / dots2 / grow / moon / clock / hourglass）；启动时按本机日志计数取模循环，也可 <code>--spin &lt;名字&gt;</code> 固定。
      </div>
      <div class="callout note">
        <span class="title">行即事实（--display A）</span>
        工作账模式下每步只有一行：运行时右侧转圈 + 已耗时 + 当前动作，完成瞬间原位变为「✓ 耗时 结论」，失败变「✗ 原因」。
        收尾时整体条撤场，账目行按序重放为普通行，让最终信息排在账目之后。底部整体进度条全程钉在屏幕最底端。
      </div>
      <div class="callout note">
        <span class="title">日志</span>
        <code>%LOCALAPPDATA%\pi-launcher\logs\pi-launcher-&lt;hex&gt;.log</code>，UTF-8 带 BOM，本地时间戳（<code>GetLocalTime</code>），每次启动新文件、自动保留最近 20 份。
      </div>
      $ui
    </section>

    <!-- main.rs -->
    <section id="main">
      <h2><span class="num">7</span>主入口</h2>
      <p>手工解析参数，正常 / 安静两条流水；<code>boot_pipeline</code> 把步骤 1-4 的错误映射为退出码；supervisor 常驻监控 pi-web。</p>
      $main
    </section>

    <!-- 运行模式 -->
    <section id="modes">
      <h2><span class="num">8</span>运行模式与参数</h2>
      <table>
        <thead><tr><th>用法</th><th>行为</th></tr></thead>
        <tbody>
          <tr><td><code>pi-launcher</code></td><td>正常模式：banner + 动画进度 + Pi CLI 交互 + 30 秒倒计时关窗</td></tr>
          <tr><td><code>pi-launcher --quiet</code>（<code>-q</code> / <code>--auto</code>）</td><td>安静模式：仅 2 行提示（含日志路径），跳过 Pi CLI，成功/失败自动关窗；supervisor 照常</td></tr>
          <tr><td><code>pi-launcher --display &lt;A|B|C&gt;</code></td><td>显示模式：A=工作账·一步一行、完成即凝固（默认）；B=轻修·分栏仪表；C=极简·一行流。别名 <code>a/b/c</code>、<code>ledger/lite/min</code> 均可，大小写不敏感；非法值退出码 1 并列出可选项；quiet 与非终端下无效</td></tr>
          <tr><td><code>pi-launcher --spin &lt;名字&gt;</code></td><td>固定右侧动画；池内可选 dots / dots2 / grow / moon / clock / hourglass</td></tr>
          <tr><td><code>pi-launcher -h</code> / <code>--help</code></td><td>打印用法与退出码表</td></tr>
          <tr><td><code>pi-launcher --force-tty</code></td><td>调试用：非终端环境强制走动画渲染路径</td></tr>
        </tbody>
      </table>
      <div class="callout info">
        <span class="title">倒计时</span>
        正常模式结束时提示「按 Enter 键关闭 · N 秒后自动关闭」；停留开始时自动清空 pi 留下的输入残键（否则残留的 Enter 会让窗口秒关），30 秒无输入自动关窗并写入日志，超时沿用本次退出码。非交互终端直接关闭。
      </div>
    </section>

    <!-- 退出码 -->
    <section id="exitcodes">
      <h2><span class="num">9</span>退出码与失败策略</h2>
      <h3>退出码</h3>
      <table>
        <thead><tr><th>退出码</th><th>含义</th></tr></thead>
        <tbody>
          <tr><td><code>0</code></td><td>全部成功</td></tr>
          <tr><td><code>1</code></td><td>环境预检失败</td></tr>
          <tr><td><code>2</code></td><td><code>pi update</code> 失败</td></tr>
          <tr><td><code>3</code></td><td><code>pi-web</code> 检查 / 升级失败</td></tr>
          <tr><td><code>4</code></td><td><code>pi-web</code> 启动失败</td></tr>
          <tr><td><code>5</code></td><td>Pi CLI 异常</td></tr>
        </tbody>
      </table>

      <h3>失败与容错策略</h3>
      <table>
        <thead><tr><th>场景</th><th>处理</th></tr></thead>
        <tbody>
          <tr><td>命令缺失（预检）</td><td>失败退出；日志含安装命令提示</td></tr>
          <tr><td><code>pi update</code> 失败</td><td>stderr 摘要进日志，退出码 2</td></tr>
          <tr><td>pi-web 正在运行 + registry 有新版本</td><td>跳过升级（进程锁定文件），按现版继续；提示 <code>pi-web stop</code> 后重跑 launcher</td></tr>
          <tr><td>升级遇 EBUSY / 文件锁</td><td>警告但不中止启动，按现版继续（真正的升级失败才用退出码 3）</td></tr>
          <tr><td>pi-web 启动失败 / 超时</td><td>终止残留子进程；退出码 4</td></tr>
          <tr><td>pi-web 运行中崩溃</td><td>supervisor 自动重启（最多 3 次），HTTP 无响应每 ~10 秒告警</td></tr>
          <tr><td>Ctrl+C</td><td>launcher 退出（码 130）；detached 的 pi-web 继续运行</td></tr>
        </tbody>
      </table>
    </section>

    <!-- 编译 -->
    <section id="build">
      <h2><span class="num">10</span>编译可执行文件</h2>

      <h3 id="build-basic">编译与安装</h3>
      <div class="code-block">
        <div class="code-header"><span class="filename">命令</span><span class="lang">SHELL</span></div>
<pre><code># 开发版
cargo build

# 发布版（target/release/pi-launcher.exe）
cargo build --release

# 编译并运行
cargo run --release

# 安装到 ~/.cargo/bin 全局可用
cargo install --path .</code></pre>
      </div>

      <h3 id="build-size">减小体积</h3>
      <div class="code-block">
        <div class="code-header"><span class="filename">Cargo.toml</span><span class="lang">TOML</span></div>
<pre><code>[profile.release]
opt-level = "z"
lto = true
strip = true
codegen-units = 1
panic = "abort"</code></pre>
      </div>

      <h3 id="build-trouble">编译排错</h3>
      <div class="callout warn">
        <span class="title">常见问题</span>
        依赖下载慢可配置 cargo 国内镜像（USTC 等）；提示找不到模块时，核对 <code>main.rs</code> 里的 <code>mod</code> 声明与 <code>src/</code> 下文件名（大小写）是否一致。
      </div>
    </section>

    <!-- 设计说明 -->
    <section id="design">
      <h2><span class="num">11</span>设计说明</h2>

      <h3>为什么所有输出都走 ui.rs</h3>
      <p>同一段步骤逻辑要在三种环境正确工作：全功能动画终端、CI 管道、计划任务。分散的 <code>println!</code> 无法统一三种表现，也无法保证「不打印输出但日志照写」。统一输出层让步骤代码完全不感知模式差异。</p>

      <h3>为什么 CLI 前要清掉进度条</h3>
      <p>Pi CLI 是全屏交互界面：任何还在重绘的 bar 都会破坏它的画面。进入步骤 5 前先整体 <code>finish_and_clear</code>，之后 supervisor 消息退化为普通 <code>println</code>（此时打印是安全的）。</p>

      <h3>为什么不伪造进度</h3>
      <p>npm 系命令的真实进度不可观测，中间过程一律只显示「已耗时」与转圈动画，不虚构百分比。唯一确定时长的场景是「等待 pi-web HTTP 就绪」（60 秒时间轴），其比例条与真实耗时一致。</p>

      <h3>升级策略（已定稿）</h3>
      <div class="callout success">
        <span class="title">文件锁处理</span>
        pi-web 正在运行 → 本次不升级，按现版继续；升级撞锁（EBUSY / -4082 / "busy or locked"）→ 警告后按现版继续。两者都保留可观测性（警告 + 日志）。想完成升级：<code>pi-web stop</code> → 再跑一次 launcher。
      </div>

      <h3>supervisor 为何两种模式都在</h3>
      <p>安静模式下没有人盯着窗口，pi-web 却一样会出问题——因此守护与重启循环不随渲染模式变化，只是消息都收敛到日志。</p>

      <h3>为什么退出码细分</h3>
      <p>计划任务的自动化不能依赖人眼：退出码细分到每个可失败步骤，日志记录到该退出码的完整路径，单独看一次日志就知道挂在哪一步。</p>
    </section>

    <footer>
      Pi Agent 批量自动化启动工具 · Rust 实现文档 · v1.1.5
    </footer>

  </main>
</div>

</body>
</html>
"@

[System.IO.File]::WriteAllText($doc, $html, [Text.UTF8Encoding]::new($false))
"OK: $doc  ($((Get-Item $doc).Length) bytes)"

