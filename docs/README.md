# 开发文档 / Development docs

User documentation lives in the [GitHub Wiki](https://github.com/mizorewww/course2md/wiki); 用户文档见 Wiki，本目录不保留第二份用户指南。

- [Desktop build and contribution guide](../desktop/README.md)
- [Packaging and release automation](PACKAGING.md)
- [Architecture: conversion engine](DESIGN.md)
- [Benchmarks](BENCHMARKS.md)
- [Open engineering follow-ups](FOLLOW-UPS.md)
- [Release notes](releases/)
- [Changelog](../CHANGELOG.md)

贡献约定（原子提交、发布规则、桌面设计权威）见 [AGENTS.md](../AGENTS.md)。合入前需要通过的检查以 [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) 和 [`desktop.yml`](../.github/workflows/desktop.yml) 为准。CLI 帮助与用户可见消息保持「中文 / English」双语，格式与 `src/cli.rs` 中的现有参数说明一致。
