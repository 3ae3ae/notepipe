# Development records / 开发文档

User documentation lives in the [GitHub Wiki](https://github.com/mizorewww/course2md/wiki); 用户文档见 Wiki，本目录不保留第二份用户指南。

## 文档索引

- [Desktop build and contribution guide](../desktop/README.md)
- [Packaging and release automation](PACKAGING.md)
- [Architecture: conversion engine](DESIGN.md)
- [Long-list virtualization design](VIRTUALIZATION.md)
- [Benchmarks](BENCHMARKS.md)
- [Engineering audit](ENGINEERING-AUDIT.md)
- [Design review record](DESIGN-REVIEW.md)
- [Release notes](releases/)
- [Changelog](../CHANGELOG.md)

## 开发纪律

面向本仓库贡献者的基本要求。桌面端构建细节见 [desktop/README.md](../desktop/README.md)，打包与发布渠道见 [PACKAGING.md](PACKAGING.md)。

### 构建与验证

CI（`.github/workflows/ci.yml`）在 Linux、macOS、Windows 上运行，合入前本地应通过同等检查：

```sh
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --features integration --test scene_synthetic
cargo build --release
```

桌面端另行执行（构建前先运行 `python3 desktop/scripts/sources.py` 准备 GPUI 依赖）：

```sh
cargo test --manifest-path desktop/Cargo.toml
```

### 双语消息

CLI 帮助与用户可见消息保持「中文 / English」双语，两种语言表达同一含义，格式与 `src/cli.rs` 中现有参数说明一致。新增或修改消息遵循同一约定。

### 提交与发布

- 按关注点拆分原子提交，保持变更可审查；一次提交只做一件事。
- 普通变更不改 release tag、不替换已发布的资产。
- 发布流程（冻结依赖、版本文件、发布说明）见 desktop/README.md 与 PACKAGING.md；发布说明存放于 [releases/](releases/)。

### 桌面 UI

所有桌面界面设计与修改必须遵循 [.agents/skills/course2md-design](../.agents/skills/course2md-design/SKILL.md)。它是当前设计权威，优先于历史文档；颜色、控件、图标与动效使用共享设计原语，不在页面内另创视觉方言。整体产品级 UI 变更需用子代理独立审查每个受影响页面及其加载、空、失败、完成状态，记录具体发现与处理结果，并验证运行中的原生应用而非仅源码或网页原型。保持普通键盘行为与既有系统偏好，不扩展为无障碍专项。
