# pi-launcher 项目规则

## 构建（重要规则，不可移除）

**每次生成目标码都必须保持程序体积优化 / 压缩，减小发布版体积。**

`Cargo.toml` 已内置以下 `[profile.release]` 配置；任何人不得移除或弱化这些设置：
后续若调整 profile，必须保持等效或更优的体积表现（改前改后在 `target/release/`
核对 `pi-launcher.exe` 的文件大小，不得明显回退）：

```toml
[profile.release]
opt-level = "z"      # 优先体积的优化级别
lto = true           # 链接期跨 crate 优化
strip = true         # 剥离符号表
codegen-units = 1    # 单编译单元，最大化优化机会
panic = "abort"      # 去 unwind 表（CLI 无需 panic 捕获）
```

参考：本规则生效时 v1.0.0 的 release 产物约 390 KB（399,360 字节）。
