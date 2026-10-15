# course2md 原生桌面端

基于 GPUI 与 GPUI Component 的 macOS、Windows、Linux 客户端。转换使用同包 CLI，支持链接和本地视频、字幕优先、各 ASR 后端、进度/取消、课程搜索、图文笔记阅读，以及共享配置文件。

## 安装与首次使用

普通用户请阅读 [GUI 安装指南](https://github.com/mizorewww/course2md/wiki/%E5%AE%89%E8%A3%85%E6%A1%8C%E9%9D%A2%E5%BA%94%E7%94%A8)（[English](https://github.com/mizorewww/course2md/wiki/GUI-Installation)）。Homebrew GUI 使用 `brew install --cask mizorewww/tap/course2md-gui`；AUR GUI 使用 `yay -S course2md-gui-bin`。首次启动可直接选择视频，需要时在设置中配置识别与 AI 服务。

本页其余内容面向开发者，无需为使用应用安装 Rust 或执行构建命令。所有用户文档见 [Wiki](https://github.com/mizorewww/course2md/wiki)。

## 开发

需要 Rust stable、Python 3.11+、Git。macOS 需 Xcode Command Line Tools；Windows 需 Visual Studio C++ Build Tools 和 LLVM；Linux 系统依赖见 `.github/workflows/desktop.yml`。处理视频需要 ffmpeg/ffprobe，远程链接还需要 yt-dlp。GPU/CPU 识别需 llama-server；Apple 原生、Intel NPU 和 API 的要求与 CLI 相同。

```sh
python3 desktop/scripts/sources.py
cargo build
cargo build --manifest-path desktop/Cargo.toml
# 开发时指定刚构建的引擎；Windows PowerShell 使用 $env:COURSE2MD_BIN
COURSE2MD_BIN="$PWD/target/debug/course2md" cargo run --manifest-path desktop/Cargo.toml
```

把当前源码装到本机测试：

```sh
just install
```

写入 `/Applications/course2md.app`，沿用现有笔记和设置。

`sources.py` 默认先 fast-forward pull `~/Developer/zed` 和 `~/Developer/gpui-component` 的 main，再建立 `desktop/.deps` 下的独立工作树。可通过 `--developer-dir` 指定仓库根目录；`--no-pull` 仅复用本次已更新的源码。开发不使用版本号或固定 commit。两个原始工作区必须干净，脚本不会替你丢弃修改。

组件主线现属于 GPUI Kit，使用重新发布的 `gpui-pre` 包名。准备脚本只在独立工作树中将依赖映射到 Zed GPUI，并让组件宏兼容原始 `gpui` 包名；不改动开发者原始工作区。上游布局变更时脚本会明确失败，要求检查兼容调整。

应用支持 `COURSE2MD_BIN`、同目录 CLI、PATH 三种引擎位置。macOS 从 Finder 启动也会补充 Homebrew 工具路径。配置位置与 CLI 相同，首次桌面使用默认保存到 `~/Documents/course2md`。

## 本机打包

```sh
python3 -m pip install -r desktop/scripts/requirements-packaging.txt
python3 desktop/scripts/package.py --debug
```

产物位于 `desktop/target/packages/`。macOS 为包含 CLI 和 MLX Metal 库的 `.app`；Windows 为两份 `.exe`；Linux 为两份可执行文件以及桌面入口。Windows/Linux 解压后保留两份程序在同一目录。本机默认 ad-hoc 签名；发布 CI 使用已有 Developer ID 与 Apple API 凭据签名、公证并生成 DMG。macOS ZIP 使用 ditto 保留签名所需的符号链接。

macOS DMG 使用 dmgbuild 固定 Finder 窗口、Retina 背景和拖动位置，窗口只显示应用与 Applications 快捷方式；许可证与源码版本信息保存在应用的 Resources 内。打包后自动挂载检查布局、图标、快捷方式与应用签名，不依赖 Finder GUI。设计与实机预览见 [安装界面](assets/dmg/README.md)。

Windows 构建将多尺寸 ICO 嵌入 GUI 程序的资源编号 `1`，与 GPUI 读取的编号一致。打包会检查成品 `.exe` 的每个图标尺寸及内容；缺失或过期会直接失败。应用图标的 Icon Composer 源文件与导出方式见 [图标设计](assets/icon-design/README.md)。

## 发布

完成开发、测试和实际界面验收后冻结**当时使用的**源码：

```sh
python3 desktop/scripts/sources.py --freeze
# 一起提交 sources.lock.json 和 desktop/Cargo.lock
python3 desktop/scripts/sources.py --locked
python3 desktop/scripts/package.py
```

发布命令核对实际工作树与冻结记录，并使用 Cargo `--locked`。普通开发命令仍继续追踪 main。CI 为三个系统分别构建；发布使用冻结记录，PR 构建使用主线。可以手动运行 release 工作流并指定版本，全部构建成功后再创建对应 tag 和 GitHub Release。

预发布使用 `2.0.0-rc.2` 等 SemVer 格式，并同步更新两个 Cargo.toml、锁文件和 `docs/releases/v版本.md`。release 工作流会设置 GitHub Pre-release，保留稳定版 Latest。Homebrew 按 `alpha`、`beta`、`rc` 更新独立通道，例如 `course2md-rc` / `course2md-gui@rc`；AUR 跳过预发布。macOS 包将完整版本保存在 `Course2mdVersion`，使用符合 Apple 格式的数字短版本及开发后缀（RC2 为 `2.0.0` / `2.0.0fc2`），应用「关于」和 CLI 显示完整 SemVer。

## 验证

```sh
cargo test --features integration
cargo test --manifest-path desktop/Cargo.toml
```

实际操作验证以真实运行的应用为准。所有桌面界面修改必须遵循 [项目设计 skill](../.agents/skills/course2md-design/SKILL.md)。开发验收使用独立 `XDG_CONFIG_HOME`，不修改个人 API 配置。

## 界面与操作

- 标题栏提供工作台、我的笔记、任务与设置。macOS 保留原生窗口控制和拖动区域。
- 工作台以链接输入栏开始，也支持选择或拖入本地视频。输入栏右侧可展开本次生成选项，最近笔记以缩略图列表呈现；缺少字幕或服务时就地提示。
- 任务显示当前步骤、真实进度或已用时间，并提供暂停、继续与取消；完成后可直接阅读笔记。后台完成提示不会打断当前页面。
- 笔记库以封面与标题网格呈现，保留列表、文件夹、搜索和显示菜单。阅读器使用居中正文列和完整截图；目录与查找按需打开，保留版本及导出。
- 设置概览分为生成笔记默认值、保存、账号与外观。服务管理、存储管理、配色与文字大小、运行检查通过相应入口进入。新笔记默认选项与单篇草稿的修改分别保存；服务测试由用户主动发起，测试结果与保存状态分别显示。
- 页面、选项展开、加载、进度与主题切换使用共享动效。外观页中的减少动态效果可立即显示最终状态。

## 长列表

阅读页文章流使用可变高 `list`，任务列表和课程库网格按行使用虚拟列表，只构建可视区内的元素。改动时要保持阅读位置的保存与恢复、页内查找定位和图片跳转；渲染期间不做文件或网络 IO。

## 主题

在设置概览选择跟随系统、浅色或深色，通过「配色与文字大小」选择配色。浅色和深色偏好分别保存，跟随系统时使用对应配色；重启后保留选择。

内置 Paper、Ink、Nord Snow、Nord、Tokyo Day、Tokyo Night，以及 Catppuccin Latte、Frappé、Macchiato、Mocha，共 10 套。主题覆盖标题栏、表单、菜单、对话框、阅读高亮和状态提示。颜色角色与来源见 [主题资源](assets/themes/SOURCES.md)。

## 界面图标

内嵌 Google Material Icons Rounded SVG，运行时无需网络或安装图标字体。
导航图标统一为 20px，任务、刷新和加载使用各自对应的图标；共享组件中的
复选框、展开箭头、密码可见性和窗口控制也使用同套资源。
来源、版本和 Apache-2.0 许可证见 [图标资源](assets/material/README.md)。YouTube 与 Bilibili 使用独立的 [原品牌标识](assets/brands/SOURCES.md)；主题与品牌资源的许可证随应用打包。

关于页及应用菜单可查看版本与构建提交。
