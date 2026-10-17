# course2md 应用图标

核心图形是「播放三角 + Markdown 向下箭头」，外加横向圆角框。
蓝色底面、半透明玻璃框和两个实心符号形成清晰层次；不加入文字、播放器控件或额外装饰。

## 文件

- `Course2MD.icon`：Icon Composer 的可编辑源文件。1024 × 1024 画布，包含 3 个 SVG 前景图层（`Assets/`）和 2 个材质分组，背景与材质由 Icon Composer 渲染。
- `export.py`：导出脚本。原生渲染图和预览板写入 `exports/`（不纳入版本库），打包用的图标写入 `desktop/assets/`。

`.icon` 中的图形保持完整画布，由系统处理外形；传统 ICNS 的主体按 824/1024 比例居中，作为静态文件的视觉留白。最终色彩、折射与高光会随系统版本和外观变化。

## 继续编辑与重新导出

双击 `Course2MD.icon`，在 Icon Composer 中修改并保存。图形分别位于「Video to Markdown」与「Glass frame」分组；边框图层不透明度为 48%。

在安装了 Xcode（含 Icon Composer）与 Pillow 的 macOS 环境运行：

```sh
python3 desktop/assets/icon-design/export.py
```

脚本调用 Apple 的 `ictool` 原生渲染器导出各外观，再生成 `desktop/assets/` 下的 `icon.icns`、`icon.ico`、`icon.png` 及小尺寸 PNG；Pillow 只负责静态排版、颜色转换和缩放。此版本使用 Icon Composer 2.0 / 115.1、设计代际 27。

macOS 使用默认外观的静态 `icon.icns`，兼容 macOS 15 起的现有应用包；`.icon` 源文件保留所有可编辑材质与外观。Windows 的 `icon.ico` 包含 16、20、24、32、40、48、64、96、128、256 像素，在 `build.rs` 中编译为编号 `1` 的资源。Linux 使用 `icon.png`。修改后应运行导出脚本，再重新构建 Windows 程序。
