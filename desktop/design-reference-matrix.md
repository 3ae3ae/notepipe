# 39 张设计参考与源码覆盖矩阵

记录日期：2026-10-14。此文件记录用户提供的原始参考、归档完整性及源码对应关系；**不是原生界面验收通过报告**。原生截图、交互检查与尚未访问的状态须另行记录。

## 参考范围与用户覆盖

- 压缩包根目录：`/Users/aac6fef/Downloads/设计稿`，共 `1.zip` 至 `13.zip`。
- 解压根目录：`/Users/aac6fef/Downloads/设计稿/解压`，各编号子目录各有 1440、1024、720 三档，共 39 张 PNG。表中“参考宽度”是文件名标注的逻辑宽度；原图像素宽度为其两倍，高度按各状态自然内容保留。
- 用户要求按参考对齐排版、信息层级、控件形状与响应布局；**颜色保持软件已有语义 palette，保留全部主题、浅色／深色选择和原生窗口 traffic lights**。不能用参考色值替换用户主题。
- 用户后续明确要求预览确认：**Enter、预览、选文件和拖入文件只读取／刷新视频信息；只有显式“开始转换”或明确标注的批量／新版生成动作建立转换意图**。参考图中的“开始整理”和拖放后直接整理的旧暗示由此覆盖。
- 保留原大标题、详细设置入口及已有功能；真实服务、登录、模型、进度、保存位置和错误状态优先于参考中的示例成功状态。参考内虚构标题、作者、时长和百分比不能填入实际应用状态。
- 第 1 组 720 图内出现的嵌入式小截图作为原始文件保留和哈希记录；它是参考合成内容，不实现为产品的浮动界面。
- 源码路径均相对本文件所在的 `desktop/`；共同设计约束见项目 `.agents/skills/course2md-design/SKILL.md`。

## 编号与源码覆盖

“源码覆盖”只说明相关实现入口可追踪，不等同于三档尺寸均已原生通过。所有组的 shell／导航还受 `src/views.rs`、共享控件受 `src/theme.rs` 与 `src/choice_group.rs` 控制。

| 编号 | 状态 | 参考宽度 | 源码对应（均在 `src/`） | 需验证的状态边界 |
|---|---|---|---|---|
| 1 | 首页 · 默认／选项展开 | 1440 / 1024 / 720 | `import_ui.rs`: `new_page`, `import_text_mode_options`, `import_ai_options`, `import_destination`, `import_exports`; `course_library.rs`: `recent_notes_section` | 展开面板与最近笔记双列／窄窗堆叠；真实模型和服务状态 |
| 2 | 首页 · 选项收起 | 1440 / 1024 / 720 | `import_ui.rs`: `new_page`, `box_source_input`; `course_library.rs`: `recent_notes_section` | 收起选项后最近笔记占满下方宽度；tune 状态与实际展开值一致 |
| 3 | 首页 · 读取链接中 | 1440 / 1024 / 720 | `import_ui.rs`: `box_source_input`; `motion.rs`: `indeterminate_progress` | 读取占位卡、无分母细进度、取消及晚回调隔离 |
| 4 | 首页 · 链接已识别 | 1440 / 1024 / 720 | `import_ui.rs`: `box_selected_video`, `box_bottom_row`, `conversion_recovery` | 真实封面／时长／字幕状态、显式开始转换、移除预览、重复来源恢复 |
| 5 | 首页 · 链接无法识别 | 1440 / 1024 / 720 | `import_ui.rs`: `box_source_input`, `source_preview_error`, `candidate_list_view` | 字段错误与紧凑恢复区；重试、登录、技术详情、候选选择保留 |
| 6 | 首页 · 拖入文件 | 1440 / 1024 / 720 | `import_ui.rs`: `workbench_drop_overlay`, `drop_source_files`; `views.rs`: `workbench-drop` group | 外部文件拖入全窗覆盖；释放只读取预览，多文件保留候选选择 |
| 7 | 首页 · 处理中 | 1440 / 1024 / 720 | `import_ui.rs`: `new_page`; `course_library.rs`: `workbench_active_tasks_section`; `task_ui.rs`: `compact_task_progress` | 全部排队／运行任务同一 surface；真实阶段与进度、取消、打开任务详情 |
| 8 | 首页 · 首次使用 | 1440 / 1024 / 720 | `course_library.rs`: `recent_notes_section` | 空库说明及三步 chips；加载／不可读库分支不得伪装为空库 |
| 9 | 设置 · 外观 | 1440 / 1024 / 720 | `settings_ui.rs`: `settings_page`, `settings_navigation`; `appearance_ui.rs`; `views.rs` | 三档竖导航、模式选择、浅深双列预览、配色选择、字号和减少动态效果 |
| 10 | 设置 · 生成笔记 | 1440 / 1024 / 720 | `generation_settings.rs`: `generation_settings_page`; `settings_ui.rs`: 共享设置行 | 文字来源、语音识别、AI 处理、保存；长期默认与本次选项分离 |
| 11 | 设置 · 服务与账号 | 1440 / 1024 / 720 | `settings_ui.rs`: `services_settings_page`; `service_editor.rs` | Bilibili、Codex、Ollama、其他 AI 服务、语音服务；真实登录和连接状态 |
| 12 | 设置 · 存储 | 1440 / 1024 / 720 | `storage_ui.rs`: `storage_settings_page`; `settings_ui.rs` | 保存位置、路径换行、打开／移动／添加课程库及错误恢复 |
| 13 | 设置 · 应用 | 1440 / 1024 / 720 | `settings_ui.rs`: `application_settings_page`; `model_diagnostics.rs` | 运行检查、模型资源、关于、许可证和用户引导；真实检测／下载状态 |

## 逐图原始文件、尺寸及校验值

SHA-256 针对完整 PNG 文件字节；CRC32 同时与相应 ZIP 成员头中的 CRC 校验值一致。绝对路径指向本次用户提供的本机原始文件，未把图像复制进仓库。

| 编号／宽度 | 状态 | 解压绝对路径 | PNG 像素尺寸 | SHA-256 | ZIP CRC32 |
|---|---|---|---|---|---|
| 1 / 1440 | 首页 · 默认／选项展开 | `/Users/aac6fef/Downloads/设计稿/解压/1/1440 · 默认.png` | 2880 × 2880 | `29bc73231661e48a4f8e6c767892f9340b1ec7be668c3cba5c79d7943b5fd7d4` | `cacb18f2` |
| 1 / 1024 | 首页 · 默认／选项展开 | `/Users/aac6fef/Downloads/设计稿/解压/1/1024 · 默认.png` | 2048 × 2920 | `b9d4e6b232ba0402580ecf8664950714482e4968d76b129e0d701c9a468f71c7` | `68d0d257` |
| 1 / 720 | 首页 · 默认／选项展开 | `/Users/aac6fef/Downloads/设计稿/解压/1/720 · 默认.png` | 1440 × 3920 | `bd8be7957203ff1280af9c4cc585a5c47cf1ee98224e7e92fe254dfb20909c1f` | `7d07034c` |
| 2 / 1440 | 首页 · 选项收起 | `/Users/aac6fef/Downloads/设计稿/解压/2/1440 · 选项收起.png` | 2880 × 1920 | `d352660720c215e8ee6731e420abeaadde27a344533594623734bc374104ebb5` | `49e84efe` |
| 2 / 1024 | 首页 · 选项收起 | `/Users/aac6fef/Downloads/设计稿/解压/2/1024 · 选项收起.png` | 2048 × 1920 | `8c28e418df6e8ebcbe66245198ab4ac7c405ca8ea0332e72fcf329001fbdb2bd` | `cfbee970` |
| 2 / 720 | 首页 · 选项收起 | `/Users/aac6fef/Downloads/设计稿/解压/2/720 · 选项收起.png` | 1440 × 2000 | `f7b301309c1577a5b78881c01e86eebe2e43ffe643739e8a37316bd9f6f2e4dc` | `f042de86` |
| 3 / 1440 | 首页 · 读取链接中 | `/Users/aac6fef/Downloads/设计稿/解压/3/1440 · 读取链接中.png` | 2880 × 2160 | `dffe203b54c7447e0f8a8d2b75b9e16e091b1222adad1a71e5195613a4f72287` | `d58142cd` |
| 3 / 1024 | 首页 · 读取链接中 | `/Users/aac6fef/Downloads/设计稿/解压/3/1024 · 读取链接中.png` | 2048 × 2160 | `a12e18adc2505155536647c8d2934ae2efc2a57417e5e6cb3ae7c7a5a40210ec` | `91333772` |
| 3 / 720 | 首页 · 读取链接中 | `/Users/aac6fef/Downloads/设计稿/解压/3/720 · 读取链接中.png` | 1440 × 2240 | `b21141ef25f9ef9f9ad68ae550be0aab100e5d3dd5e208513e9eb85fa2a1a3a4` | `6107ee21` |
| 4 / 1440 | 首页 · 链接已识别 | `/Users/aac6fef/Downloads/设计稿/解压/4/1440 · 链接已识别.png` | 2880 × 3120 | `b0e781486268dee6de5b76fbd6e45fe5b091b73061b9c7dea61ecc2eea8ad798` | `3b2c8ad0` |
| 4 / 1024 | 首页 · 链接已识别 | `/Users/aac6fef/Downloads/设计稿/解压/4/1024 · 链接已识别.png` | 2048 × 3160 | `6ba729a4ebf835e093ec1d010ea18582b12224d49678885f557385138f776d15` | `9066fd21` |
| 4 / 720 | 首页 · 链接已识别 | `/Users/aac6fef/Downloads/设计稿/解压/4/720 · 链接已识别.png` | 1440 × 4160 | `401a7db36d73dccc568b15a873d3f9b73935a971cb139e9fa5591657ee4d71f3` | `36ec1875` |
| 5 / 1440 | 首页 · 链接无法识别 | `/Users/aac6fef/Downloads/设计稿/解压/5/1440 · 链接无法识别.png` | 2880 × 2000 | `d8fbbbf2e2a79809381d98d067e2c71aa6179bf8ed6a37c28cf2ef2eda2130d6` | `0aa68b6b` |
| 5 / 1024 | 首页 · 链接无法识别 | `/Users/aac6fef/Downloads/设计稿/解压/5/1024 · 链接无法识别.png` | 2048 × 2000 | `9c2e01b23e49b7180aeef3678f466aa36fb21cc2fe199ee7f6a4bcfdc5ee1732` | `353dfe8e` |
| 5 / 720 | 首页 · 链接无法识别 | `/Users/aac6fef/Downloads/设计稿/解压/5/720 · 链接无法识别.png` | 1440 × 2080 | `2e40550ad41291f3b0ecdf157a20a88e5f136828e6c7ed4b68d57e75b69a7ad9` | `97ee24a6` |
| 6 / 1440 | 首页 · 拖入文件 | `/Users/aac6fef/Downloads/设计稿/解压/6/1440 · 拖入文件.png` | 2880 × 1920 | `d851b310f9411e770802d4637dbdc40e5e4e9af93035202a0aa158b1f77fee79` | `d9d70a48` |
| 6 / 1024 | 首页 · 拖入文件 | `/Users/aac6fef/Downloads/设计稿/解压/6/1024 · 拖入文件.png` | 2048 × 1920 | `d2f8dc621b721affde3660444034997e1c92cf86380ea105b358d1f752cf85b5` | `b38807a0` |
| 6 / 720 | 首页 · 拖入文件 | `/Users/aac6fef/Downloads/设计稿/解压/6/720 · 拖入文件.png` | 1440 × 2000 | `be605ed72ad4bdf350955beeb54f0968743b12e171b1f7e9b4aa849a1f81ddca` | `e2d1aa42` |
| 7 / 1440 | 首页 · 处理中 | `/Users/aac6fef/Downloads/设计稿/解压/7/1440 · 处理中.png` | 2880 × 2400 | `7db01f57f4ebbd2ebf043850c7b6ac95d6e51b2533ceea099cd1145dbcc5697a` | `d1842a05` |
| 7 / 1024 | 首页 · 处理中 | `/Users/aac6fef/Downloads/设计稿/解压/7/1024 · 处理中.png` | 2048 × 2400 | `553e0714f18be1734989e4a95b8b4243bd32a019cb6105962be485c283bff052` | `8d617c9d` |
| 7 / 720 | 首页 · 处理中 | `/Users/aac6fef/Downloads/设计稿/解压/7/720 · 处理中.png` | 1440 × 2560 | `28e1ea18c4ee4a05ec82e43a5dd47aef0715099e9b88204ba1630574cc404bb4` | `25384f10` |
| 8 / 1440 | 首页 · 首次使用 | `/Users/aac6fef/Downloads/设计稿/解压/8/1440 · 首次使用.png` | 2880 × 1800 | `6176a3b70b92718d3693f07357d170be4298096143729c393bbfdf9bfeff91f5` | `662f5c63` |
| 8 / 1024 | 首页 · 首次使用 | `/Users/aac6fef/Downloads/设计稿/解压/8/1024 · 首次使用.png` | 2048 × 1800 | `3688293b34bebd8f90cc65da3d8a12aab4b28c58ec4b469d76557dcbaf09b515` | `89fde416` |
| 8 / 720 | 首页 · 首次使用 | `/Users/aac6fef/Downloads/设计稿/解压/8/720 · 首次使用.png` | 1440 × 1880 | `efb0558fdd1912770858c1c0b7fd1150b8ccaec4c84a683e3f94229929556ce2` | `e51a3f0e` |
| 9 / 1440 | 设置 · 外观 | `/Users/aac6fef/Downloads/设计稿/解压/9/设置 · 外观 · 1440.png` | 2880 × 1840 | `c21e24e08bb6fdd43b97acbfba348f41e495a6b595979c870bec603000d0b8db` | `481d6277` |
| 9 / 1024 | 设置 · 外观 | `/Users/aac6fef/Downloads/设计稿/解压/9/设置 · 外观 · 1024.png` | 2048 × 1840 | `460700a67fd08b385d3473c8ab24ed8d2b831745153b9810262969a72766ee9f` | `278246d6` |
| 9 / 720 | 设置 · 外观 | `/Users/aac6fef/Downloads/设计稿/解压/9/设置 · 外观 · 720.png` | 1440 × 1920 | `dcb4e0f0196e117ae11f2bba7662ea243c984a9fa632672e360686a8375e9ed4` | `945583ce` |
| 10 / 1440 | 设置 · 生成笔记 | `/Users/aac6fef/Downloads/设计稿/解压/10/设置 · 生成笔记 · 1440.png` | 2880 × 3120 | `a6171e40cd9875ac2c36913e8ae3db6419984aabfb06e984b1a49058e1049000` | `d3053325` |
| 10 / 1024 | 设置 · 生成笔记 | `/Users/aac6fef/Downloads/设计稿/解压/10/设置 · 生成笔记 · 1024.png` | 2048 × 3160 | `feb5e343b776e5cb1648f5fddabccc382afdac8711be6472e076b9c7468b4e02` | `dbf7100c` |
| 10 / 720 | 设置 · 生成笔记 | `/Users/aac6fef/Downloads/设计稿/解压/10/设置 · 生成笔记 · 720.png` | 1440 × 3440 | `4ad0c047de3c913dbba073a47c7df478d2db8419fffd5d7cb2cb2dd6a320cf7e` | `a9d3d47d` |
| 11 / 1440 | 设置 · 服务与账号 | `/Users/aac6fef/Downloads/设计稿/解压/11/设置 · 服务与账号 · 1440.png` | 2880 × 1880 | `80c72db3f7b3a04139e704004df2799561a18817dd13b2a39dd418dccc79117f` | `6e00d2d9` |
| 11 / 1024 | 设置 · 服务与账号 | `/Users/aac6fef/Downloads/设计稿/解压/11/设置 · 服务与账号 · 1024.png` | 2048 × 1960 | `8bb7050ec749c82b9e1ecff3ec30ff1a4bb190acdd13163bd23f44977e7c7f50` | `b71f6335` |
| 11 / 720 | 设置 · 服务与账号 | `/Users/aac6fef/Downloads/设计稿/解压/11/设置 · 服务与账号 · 720.png` | 1440 × 2240 | `0ecb4d53f7a8629821573996a55f84d6c3a306a99a707760255fb913916af4a0` | `20315ae2` |
| 12 / 1440 | 设置 · 存储 | `/Users/aac6fef/Downloads/设计稿/解压/12/设置 · 存储 · 1440.png` | 2880 × 1280 | `e2904e0b7d20c7a20a1c3b9cacbd46e358edd41d15865cd93e0e55f04f6e6430` | `f3167bd5` |
| 12 / 1024 | 设置 · 存储 | `/Users/aac6fef/Downloads/设计稿/解压/12/设置 · 存储 · 1024.png` | 2048 × 1280 | `bb7d781c42cd33aad91b138522d16c01bff20b4d65688af933091ff3ad8eb2b6` | `2f5dd672` |
| 12 / 720 | 设置 · 存储 | `/Users/aac6fef/Downloads/设计稿/解压/12/设置 · 存储 · 720.png` | 1440 × 1360 | `af24b151f949617b254e8ec62df4e623fec81436a86cf0c8ad7afc3917aef49e` | `f466e30d` |
| 13 / 1440 | 设置 · 应用 | `/Users/aac6fef/Downloads/设计稿/解压/13/设置 · 应用 · 1440.png` | 2880 × 2280 | `0ff179bf73b670b32d9bd5bf704d9f853e49c8c5befbd2510f1e5a226140ed26` | `47ba6195` |
| 13 / 1024 | 设置 · 应用 | `/Users/aac6fef/Downloads/设计稿/解压/13/设置 · 应用 · 1024.png` | 2048 × 2320 | `e29b5d02de34bb410210b530094262f0d33ebd6f41443fecca664d0d3816adf8` | `70fd9076` |
| 13 / 720 | 设置 · 应用 | `/Users/aac6fef/Downloads/设计稿/解压/13/设置 · 应用 · 720.png` | 1440 × 2520 | `094e5fd5d1aa5e12251feeb3d0c135f94a2091cd6ab1fb657717c8f9fb3f9646` | `99bcc6be` |

## ZIP 完整性与 readback

本次检查执行了以下实际校验：

1. 枚举到恰好 13 个编号归档，每个归档恰好包含三个 PNG 文件；各组的三个文件名与解压子目录集合完全相同。
2. 对全部 ZIP 调用 Python `zipfile.ZipFile.testzip()`，均返回 `None`，表示完整读取并通过归档成员 CRC 检查。
3. 再逐成员 `ZipFile.read()`，与磁盘已解压文件的 `read_bytes()` 做完整字节比较；**39 / 39 完全一致**，并重新计算 CRC32 与成员 CRC 相等。
4. 验证 PNG 签名、读取 IHDR 宽高、使用 Pillow 对全部图像执行 `verify()` 并重新打开 `load()` 完整解码、计算每张图 SHA-256；参考宽度均满足 `PNG 宽度 = 文件名宽度 × 2`。
5. 枚举解压树 `*/*.png`，与核对的 39 个路径集合完全相同；无额外或遗漏 PNG。

这些检查证明归档／解压文件完整一致，不证明界面布局或交互已经正确。以下 ZIP SHA-256 可用于以后确认是否仍为同一份参考包。

| 编号 | ZIP 绝对路径 | SHA-256 | 成员／readback |
|---|---|---|---|
| 1 | `/Users/aac6fef/Downloads/设计稿/1.zip` | `40ecd93b8f8b560904def75e880687e3928b372e7967a2f58a83cf9c4567f128` | 3 / 3，CRC 与完整字节一致 |
| 2 | `/Users/aac6fef/Downloads/设计稿/2.zip` | `d7e107cb21aa56451b8c66b4d3533ad828fdd9a1357f896702af4208687fc9bb` | 3 / 3，CRC 与完整字节一致 |
| 3 | `/Users/aac6fef/Downloads/设计稿/3.zip` | `35855b188df693d8733190800ed0addcb9f5f2a7c5f00f20232e44b9ec478543` | 3 / 3，CRC 与完整字节一致 |
| 4 | `/Users/aac6fef/Downloads/设计稿/4.zip` | `7f26c0d704a9cbf1e70d77f88d6275e946158495c73db8ceeeab5715d69c2a13` | 3 / 3，CRC 与完整字节一致 |
| 5 | `/Users/aac6fef/Downloads/设计稿/5.zip` | `a717b66320c3ab0ab9e605ed9280dda35c888cd6698f52ba91a47928d8c4f483` | 3 / 3，CRC 与完整字节一致 |
| 6 | `/Users/aac6fef/Downloads/设计稿/6.zip` | `8b9cc5d5fdd8241baac4dde40542e417eb23e3d6876b664c394460959a67db89` | 3 / 3，CRC 与完整字节一致 |
| 7 | `/Users/aac6fef/Downloads/设计稿/7.zip` | `dba48ae4844992025bcb05a84ff7646389ba199f511f0a2965bc3c5dfb6fdb63` | 3 / 3，CRC 与完整字节一致 |
| 8 | `/Users/aac6fef/Downloads/设计稿/8.zip` | `0557e82e7173013daee88e3af20fc1c28dbedf5a9f55b95d513df5908ef7bc30` | 3 / 3，CRC 与完整字节一致 |
| 9 | `/Users/aac6fef/Downloads/设计稿/9.zip` | `56d3f2ca045ec78e7db0bf0c96bb19a83a795e476898f03a6a4ee9da9ef9b892` | 3 / 3，CRC 与完整字节一致 |
| 10 | `/Users/aac6fef/Downloads/设计稿/10.zip` | `94fdbbe92842eafe17a7fb17c0d002caf4b7e59074d0f3ba2b39655353289bc9` | 3 / 3，CRC 与完整字节一致 |
| 11 | `/Users/aac6fef/Downloads/设计稿/11.zip` | `6069817991183c9f381fdd6bd6ae175029caeace0372c7386b76b269a6dcc866` | 3 / 3，CRC 与完整字节一致 |
| 12 | `/Users/aac6fef/Downloads/设计稿/12.zip` | `2adf1ed62966368cf5fc4a70d8b178147c01b0ac62bc54f907d79d880a8115e5` | 3 / 3，CRC 与完整字节一致 |
| 13 | `/Users/aac6fef/Downloads/设计稿/13.zip` | `6b46f20159355c0d8c1748caa4d50212abb06153792ba96e95fd4b78d0c99338` | 3 / 3，CRC 与完整字节一致 |

## 验收声明边界

本文件不把 source review、编译、单元测试、文件尺寸或 CRC 通过当成原生验收。完成验收需要对应实际运行版本的宽／中／窄截图，并检查正常、加载、空态、失败、完成、恢复与取消，以及原有主题和动态效果偏好。未附原生证据的单元格保持“源码对应”含义，不标记为已通过。
