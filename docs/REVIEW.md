# 代码审查收口

本文件只保留仍需处理的审查项；已完成项已从清单删除，历史见 Git 提交。

## 未完成项

无。

## 原生验证矩阵

在隔离的 `desktop/scripts/ux_fixture.py` 应用中逐项检查，正常操作均从重新启动后的状态开始：

- 窗口：760×700、1180×840、1440×900。
- 字号：100%、125%、150%、200%（`font_scale` 1.0/1.25/1.5/2.0）。
- 配色：浅色 Paper、Nord Snow、Tokyo Day、Catppuccin Latte；深色 Ink、Nord、Tokyo Night、Catppuccin Frappe、Catppuccin Macchiato、Catppuccin Mocha。
- 页面：首次设置（识别方式、AI 服务、Bilibili、模型）、主页、设置（外观、创建笔记、服务与账号）、笔记库、阅读页、截图画廊。
- 状态：空笔记库、链接读取中、无效链接提示、预览完成、转录完成、截图页与阅读页正常；加载态与失败态都从界面实际触发。
- 输入：链接输入、预览、开始转换、设置页导航与字号/配色切换。

发现的界面问题与处理：

1. 150%/200% 字号下设置侧栏英文标签在 64px 轨道内断词。侧栏在放大字号时使用 72px 逻辑宽度并允许按词换行，保留图标 + 可见标签；760、1180、1440 宽度复检通过。
2. 宽窗口下侧栏标签曾被 `w_full` 推到轨道中央。仅窄轨标签使用整行居中，宽侧栏恢复图标紧邻标签。
3. 英文标签大小写/语法统一：Appearance mode、Subtitle language、Local file、Light、Subtitles will be used directly。

复现命令：

```sh
python3 desktop/scripts/ux_fixture.py prepare
python3 desktop/scripts/ux_fixture.py bundle --label review --window 1180x840
# 启动后设置页逐项切换配色与字号；截图、朗读页与链接加载/失败态按上述矩阵检查。
```
