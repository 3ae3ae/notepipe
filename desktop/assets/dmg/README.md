# course2md 安装窗口

安装窗口沿用 macOS 常见的「应用 → Applications」布局：浅灰背景、系统字体、单一方向箭头，以及中英文拖动提示。760 × 540 的窗口为用户开启的 Finder 标签栏、路径栏、状态栏留出空间。应用和文件夹使用 Finder 中真正可拖动的项目，背景只包含文字与箭头。

![Finder 实机预览](preview.png)

## 方案

对比了两个 create-dmg 项目与 dmgbuild：

- [sindresorhus/create-dmg](https://github.com/sindresorhus/create-dmg)：现成的简洁安装窗口是视觉参考。
- [create-dmg/create-dmg](https://github.com/create-dmg/create-dmg)：支持定制，但美化步骤依赖 Finder AppleScript，CI 需要额外处理图形会话。
- [dmgbuild](https://dmgbuild.readthedocs.io/en/latest/)：最终采用。直接写入 Finder 的 `.DS_Store` 布局，Python 打包脚本和无 GUI 的 CI 都可使用。

`desktop/scripts/dmg.py` 配置窗口、图标位置和 Applications 链接；`background.tiff` 同时包含 1× / 2× 图像，`background@2x.png` 用于预览。重新渲染背景只需 macOS 的 AppKit 和 Swift：

```sh
swift desktop/assets/dmg/render.swift
python3 -m pip install -r desktop/scripts/requirements-packaging.txt
python3 desktop/scripts/package.py --debug --no-build
```

`--no-build` 复用已构建的本机二进制。发布时沿用原有签名、公证、staple 顺序；dmgbuild 使用 ditto 复制应用。DMG 根目录只显示应用和 Applications，许可证与构建来源放入已签名应用的 Resources，ZIP 继续包含开发说明。
