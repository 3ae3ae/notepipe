# course2md 安装窗口

DMG 使用常见的「应用 → Applications」布局：浅灰背景、系统字体、一个方向箭头，以及中英文拖动提示。窗口为 760 × 540，给 Finder 的标签栏、路径栏和状态栏留出空间。应用和 Applications 是真正可拖动的项目，背景里只有文字和箭头。

![Finder 预览](preview.png)

`desktop/scripts/dmg.py` 使用 [dmgbuild](https://dmgbuild.readthedocs.io/en/latest/) 直接写入 Finder 布局，不需要图形会话，可以在 CI 中运行。它负责窗口、图标位置和 Applications 链接，打包后还会挂载成品检查布局与签名。`background.tiff` 同时包含 1× 和 2× 图像，`background@2x.png` 仅用于预览。

重新渲染背景并打包（需要 macOS）：

```sh
swift desktop/assets/dmg/render.swift
python3 desktop/scripts/package.py --debug --no-build
```
