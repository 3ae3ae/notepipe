# course2md 工程审查报告（v2.0.0-rc.3 基线）

- 日期：2026-09-14
- 范围：全仓库——`src/`（引擎+CLI）、`desktop/`（GPUI 桌面端）、`native/apple-asr`、构建/打包/CI 脚本。
- 方法：7 路并行只读审查（引擎核心、引擎支撑、desktop 三大页面、desktop 页面二组、desktop 页面三组、desktop 共享原语、native/构建），每路完整通读负责区域；另有主代理在 macOS 真机（M3 Max）上对 GPU 识别链路做了运行态复现。
- 分类约定：**[hack]** 权宜之计/硬编码/与文档不符；**[抽象不足]** 应共享未共享/魔法值散落；**[过度抽象]** 无谓分层/死参数化；**[工程化缺口]** 错误处理/取消/测试/IO 纪律缺失。
- 处置列：**待处置** → 本报告发布时的初始状态；随修复推进更新为 **已修复（commit）** 或 **后续建议**。
- 2026-09-27 抽查复核（当前 HEAD = v2.0.0-rc.6）：H10（a11y.rs 确不记录 panic payload）、asr.rs 超时统一走 `runtime::run_bounded`、`CLOUD_PROVIDER_INDEX`/`asr_provider_from_index` 单一映射、`crate::provider_label` 单一出处——均与代码一致，「已修复」标注可信。

## 统计

| 区域 | 高 | 中 | 低 |
| --- | --- | --- | --- |
| 引擎核心（asr/pipeline/llm/fetch/dispatch/config） | 2 | 10 | 7 |
| 引擎支撑（其余 src/） | 0 | 7 | 13 |
| desktop 三大页面（reader/settings/task） | 3 | 11 | 8 |
| desktop 页面二组（import/workspace/onboarding/preferences） | 2 | 9 | 9 |
| desktop 页面三组（库/存储/诊断/导航） | 2 | 10 | 6 |
| desktop 共享原语与支撑层 | 2 | 8 | 5 |
| native/构建/打包/CI | 1 | 4 | 3 |
| 真机复现（主代理） | 1 | 1 | 0 |
| **合计** | **13** | **60** | **51** |

## 高严重度（13 + 1 复现）

| # | 分类 | 位置 | 问题 | 处置建议 | 处置 |
| --- | --- | --- | --- | --- | --- |
| H1 | 工程化缺口 | src/asr.rs:237（超时常量 asr.rs:29） | `wait_ready` 等 llama-server 就绪最长 300s，循环不读 control；桌面取消后 GPU 首次加载仍空等，子进程不被提前 kill | wait_ready 每轮 sleep 前读 control，非 run 即 kill 并返回取消 | 已修复 0bddad4（含回归测试） |
| H2 | 工程化缺口 | src/fetch.rs:618 + src/pipeline.rs:632-645 | yt-dlp 下载及重试循环全程不读 control，协作式取消在下载期间无效 | 每次 spawn 前 check_control；取消时 kill 当前 yt-dlp 且不再重试 | 已修复 bdd1b55（含重试分类） |
| H3 | 工程化缺口 | desktop/src/reader_ui.rs:2414/1667-1768/3628-3673 | reader_page 渲染路径做 IO（canonicalize/read_dir/image_dimensions）、重建 ListState、清空查找框、每帧 Rc::default() | 加载/重置移到命令路径，render 只读快照 | 已修复 896ed88（加载移到两处 preview 变更事件；clear_find 保留窗口绑定 flag；ListState/布局布线随 H5 结构拆分处理） |
| H4 | 工程化缺口 | desktop/src/settings_ui.rs:786-788 | settings_page 每次绘制 hydrate 输入框并 spawn 模型诊断检查 | 进入设置/快照变化时执行，渲染无副作用 | 已修复 87a73c2（prepare_settings_view 事件入口） |
| H5 | 抽象不足 | desktop/src/reader_ui.rs:2396-4078 | reader_page 约 1680 行巨函数，空态/失败/完成不可单测 | 按区域拆纯函数（toolbar/meta/banner/article/gallery/toc） | 后续建议（随设计评审的 reader 改版一并拆分，避免双重改动） |
| H6 | 抽象不足 | import_ui.rs:2015 等 + main.rs:80-88 + workspace.rs:101 | 识别引擎三套并行编码（usize 索引 / &str id / AsrProvider 枚举），"5 即云端" 隐式契约散落 4 文件 | 集中索引↔枚举映射或直接持有 Option<AsrProvider> | 部分修复 a315e34（CLOUD_PROVIDER_INDEX + uses_cloud_provider + asr_provider_from_index 单一映射；Option<AsrProvider> 深改涉及 workspace 序列化 schema，后续建议） |
| H7 | 工程化缺口 | import_ui.rs:2623、onboarding.rs:1399/1986、views.rs:333/372/406 | new_page/setup_* 在渲染中启动模型文件系统检查并在渲染期改写 model_preparation 状态，违反 SKILL.md "渲染期不做 IO" | 检查触发移到事件入口；渲染期状态收敛移到事件/cx.defer | 已修复 218896c（defer 登记 + 事件同步） |
| H8 | 工程化缺口 | desktop/src/main.rs:745-746 + course_library.rs:835-837 | 进入课程库即 refresh 且 loading 时整页替换为「正在读取笔记…」，已在内存的列表闪空 | 刷新期间保留旧列表 + 非破坏性指示 | 已修复 7d310c5 |
| H9 | hack | course_library.rs:2062-2065、main.rs:1278-1281 | 「查看任务」/打开笔记成功直接写 this.page，绕过 navigate() 的草稿保存/阅读位置/跟随清理 | 全部走 navigate()（或抽 leave_page） | 已修复 4970060 |
| H10 | hack | desktop/src/a11y.rs:8-39 | 文档承诺 panic payload（含用户数据）omitted，实现却把 payload 原文写日志——文档与实现矛盾且违背隐私承诺 | 删除 payload 记录，只留 location+backtrace | 已修复 c00d6bd |
| H11 | hack | desktop/src/views.rs:598-646（同 import_ui.rs:3021-3026 等） | 测试 include_str! 读自身源码做字符串断言，测源文本而非行为 | 删除源码嗅探测试，改行为/布局断言 | 已修复 8fc6f4b（连同恒值函数机制一并清除） |
| H12 | 工程化缺口 | build.rs:13-14 | rerun-if-changed 漏 Package.resolved，swift 依赖更新后本地构建静默链旧静态库 | 补一行 rerun-if-changed | 已修复 c00d6bd |
| H13 | 工程化缺口 | 真机复现：settings 引擎选择 + choice_group | macOS 安装版（2.0.0）中选择 GPU 不持久：点击仅聚焦不提交，随后无关键盘/滚动事件把选择静默改为 coreml 并落盘（generation.json rev27→28 实证）；用户视角即「不识别 GPU/选了没用」。HEAD 行为待复测 | 在 HEAD 复现定位；修复提交/回退链路并加回归测试 | 已在 HEAD 验证修复：GPU 选择即时持久化（generation.json rev 29）并正确渲染单模型行，无回退；安装版 2.0.0 的该行为由 2.0.0→rc.3 间的设置重构修复。配套加固：73b2633（桌面 GPU 探测超时 5s→20s 与引擎对齐）、65252f6（doctor 不再把 BLAS 当 GPU） |
| H14 | hack | src/asr.rs:746 parse_gpu_devices | doctor 把 `BLAS: Accelerate`（CPU 后端行）列为 GPU 设备——任何 name: desc 行都被收，不过滤 CPU-only 设备 | 过滤已知非 GPU 前缀（BLAS/CPU 等），与 desktop backend.rs 的 MTL/CUDA/Vulkan/SYCL/ROCm 白名单对齐为共享判定 | 已修复 65252f6（共享 is_gpu_device_id 白名单，两端共用；含回归测试） |

## 中严重度（60 条，按区域）

### 引擎核心（10）
- [工程化缺口] src/asr.rs:951/1234/324-349 — ffmpeg_vad/cut_wav 无超时、非 ManagedChild、未 stdin(null)，ffmpeg 挂死会卡住取消路径。｜已修复 00452d8（共享 run_bounded：stdin 关闭+超时+ManagedChild）
- [工程化缺口] src/pipeline.rs:346-360 + fetch.rs:618-620 — 补做截图路径 download 见文件即 Ok 并 save_file_digest，把未校验文件标为已校验。｜已修复 9947969（走 prepare_cached_media）
- [工程化缺口] src/asr.rs:945-948 — 本地 transcribe_file 空文本 bail 致整次 ASR 失败；云端同情形当静音完成。｜已修复 1084970
- [hack] src/config.rs:491-500 — resume 注释「默认关闭」与实现 `unwrap_or(true)`、测试断言三者矛盾。｜已修复 feb89cc
- [hack] src/llm.rs:732-735 — test_connection 空 key 也发 `Bearer ` 头，且不走 dispatch::receive。｜已修复 c5a9958（redirects(0)+空 key 不发头）
- [抽象不足] src/fetch.rs:624-669 vs 707-735 — 下载对任意错误（含 401/404）重试 3 次，探测只重试 412。｜已修复 bdd1b55（412 专用退避/确定性错误立即返回）
- [工程化缺口] src/pipeline.rs:651 + asr.rs:72-77/247 — 截图与转写 join! 一路失败不取消另一路；阻塞 HTTP 120-300s 只在请求前 check_control。｜已修复 44ac0ac（两路各加 watch_control 竞速；阻塞 HTTP 由 120-180s 超时兜底，已注释）
- [工程化缺口] src/llm.rs:673-682/191-210 — 重试 backoff sleep 不看 control；vision 读图+base64 无取消点。｜已修复 c5a9958
- [工程化缺口] src/dispatch.rs:416 — 每次 send 前 read_dir 全量解析收据，N 段云端 ASR 为 O(N²) 磁盘扫描。｜后续建议（需要 Ledger 索引设计；仅影响长课时云端路径的性能）
- [hack] src/config.rs:196/589 vs pipeline.rs:53 — out_root 注释/course_dir() 描述的目录布局与实际不符。｜后续建议（文档修正，低风险）

### 引擎支撑（7）
- [抽象不足] doctor.rs:60-77 vs apple.rs:117-138 — metallib 检测位置清单两处不一致，结论可互相矛盾。｜已修复 289d91b（共享 find_metallib）
- [hack] models.rs:24-37 — normalize_apple_model 用 contains 猜模型名，未知输入被静默误映射。｜已修复 feb89cc（精确别名表+回归测试）
- [工程化缺口] artifact.rs:290-380 — publish 是 async fn 却全程阻塞 std::fs。｜已修复 f579c92（spawn_blocking；无 reactor 回退同步）
- [抽象不足] config.rs:558/doctor.rs:96/wizard.rs:168 — NPU 设备路径硬编码三处；doctor 仅 linux 检查 NPU。｜已修复 3a4e0c0（NPU_DEVICE_PATH；doctor 的 Windows NPU 展示后续建议）
- [hack] main.rs:50-53 — clap 解析前手工扫 argv 找 --json/run-task。｜已修复 04a8e5b（注释声明近似语义）
- [抽象不足] wizard.rs:237/245 vs settings.rs:72-74/216-219 — 云端默认 base_url/model 字面量写三份。｜已修复 3a4e0c0（向导读 AsrApi::default()；settings.rs 注释模板为文档性拷贝，后续建议同步机制）
- [工程化缺口] models.rs:345 — 2.4GB 模型下载失败留 .part 但永不续传。｜后续建议（Range 断点续传属于功能增强；当前已保证不完整文件不发布）

### desktop 三大页面（11）
- [hack] settings_ui.rs:193-218/372-381 — setting_label/settings_detail_group 用中文标签子串猜图标。｜设计任务处理（调用点显式传 Icon，随设置页设计改版）
- [hack] reader_ui.rs:2912-2915/4113-4143 — 目录选中态手写 ACCENT_SOFT 覆盖共享控件。｜设计任务处理（随 reader 改版抽 selected_row/toggle_chip）
- [抽象不足] task_ui.rs:2251-2678/3048-3184/3186-3393 — 暂停/取消/继续等动作块在队列卡与工作台各写一遍。｜设计任务处理（抽 TaskActions/task_recovery_block）
- [工程化缺口] task_ui.rs:1414-1472 — start_next_task 先标 Running 再启动进程，启动失败且补偿失败会留下永不调度的 Running；补偿错误被 let _ 吞。｜已修复 308e4f6（补偿失败时内存同步 NeedsAttention + workspace_error；recover() 重启兼底）
- [工程化缺口] task_ui.rs:1963-1976 + settings_ui.rs:4098-4101 — finish_task/添加课程库在 UI 线程做 cover 保存/canonicalize/read_to_string。｜后续建议（随任务卡重构一移 background_executor）
- [hack] settings_ui.rs:1250/3936 — Textarea 固定 px 高度多行面。｜设计任务处理（随设置页设计改版）
- [抽象不足] settings_ui.rs:1556-1566/1865-1876 — 「每 service 最新 version」BTreeMap 折叠复制两处。｜已修复 274db07（preferences::latest_versions）
- [工程化缺口] reader_ui.rs:1902 — 重新定位原视频的 probe 取消标志永不置位。｜已修复 cc62af5
- [工程化缺口] reader_ui.rs:1344/3898/4581 — 图片 fallback 共用 ElementId "failed-reader-image"。｜已修复 cc62af5
- [工程化缺口] task_ui.rs:2028-2095 — queue_page 每帧 clone 全部 TaskRecord，绘制时写 entered。｜后续建议（Rc 快照优化；任务少时无感）
- [抽象不足] settings_ui.rs:2422-3059 — service_editor_content 约 640 行单函数。｜设计任务处理（随服务编辑设计改版拆分）

### desktop 页面二组（9）
- [hack] import_ui.rs:3021-3026/3754-3786/3830-3855 — include_str! 自身源码断言。｜已修复 8fc6f4b
- [过度抽象] import_ui.rs:74-91/107-109/2111-2113/2735 — 三个恒值「配置」函数拖死分支（unreachable!()）。｜已修复 8fc6f4b
- [抽象不足] import_ui.rs:616-619 vs 1333-1336 — 错误摘要分类（WARNING:/ERROR: 子串）两处重复。｜后续建议（随错误面结构化一并处理）
- [抽象不足] import_ui.rs:668-761 vs 832-934 — 两个字幕作业的大段取消/代际守卫样板复制。｜后续建议（抽共享作业原语；竞态风险已记录）
- [hack] onboarding.rs:438-456 — model_preparation_phase 靠进度消息子串嗅探推断阶段。｜信息展示任务处理（结构化阶段事件/关键词集中）
- [抽象不足] onboarding.rs:184 等 9 处 + 5 文件 — 默认模型 "qwen3-1.7b" 硬编码散落。｜已修复 3a4e0c0（DEFAULT_ASR_MODEL）
- [抽象不足] main.rs:80-88/import_ui.rs:2184-2192/onboarding.rs:208-217/model_diagnostics.rs:73-81 — 同一 AsrProvider 展示标签四个出处。｜已修复 62b29f7（crate::provider_label）
- [抽象不足] preferences.rs:928-955 vs 956-983 — save_generation/save_application 的 intent 恢复逻辑逐字重复两遍。｜后续建议（泛型助手；行为已有测试兜底）
- [抽象不足] workspace.rs:353-355/1140-1142 — course-{digest[..32]} 目录名重复三处。｜已修复 4a5f99c（course_dir_name）

### desktop 页面三组（10）
- [工程化缺口] notes.rs:21-32/45-54 — Course::from_completed 吞 manifest 读失败，storage_dir 回退逻辑致文件夹归属丢失。｜后续建议（完成路径要求可读 manifest）
- [hack] course_library.rs:390-392 + main.rs:1279 — apply_course_title_aliases 追加不清空，诊断重复累积。｜已修复 cc62af5（幂等合并）
- [抽象不足] main.rs:1040-1045 + model_diagnostics.rs:657-670 — 模型下载进度与任务进度路径分裂：诊断面板 1s 合帧 vs 任务 250ms；stage 过滤用字符串启发式；两处文案不一。｜信息展示任务处理
- [抽象不足] model_diagnostics.rs:231-237/250-278/793-798 — preparing/result 全局一份且完成后不清 preparing。｜部分修复 218896c（onboarding 结果按 key 同步）；preparing 清理随信息展示任务收尾
- [抽象不足] model_diagnostics.rs:64-72 vs activity.rs:278-287 — 两套 bytes()，<1MB 显示「512000 字节」。｜已修复 cebda60（共用 activity::bytes）
- [工程化缺口] credentials.rs:235 — 非 macOS 凭据 load 失败 unwrap_or_default 当空表，后续 insert 覆盖全文件丢密钥。｜已修复 308e4f6（损坏拒绝写入+回归测试）
- [工程化缺口] storage.rs:315-333 — pending_journals 读失败静默跳过，半残 journal 不可续传/放弃。｜已修复 41acdf3（corrupt_journals 可见化）
- [工程化缺口] library_ui.rs:147-156 — 源探测回调绑 cx.windows().first()，窗口已关则 preview_workers 永不递减。｜已修复 41acdf3
- [抽象不足] main.rs:203-204/library_ui.rs:208-210/347-351 — 未分类三义（Some(0)/None/记录缺席）；save_folder 直接改 page 不走 navigate。｜后续建议（枚举化；save_folder 已随 H9 模式记录）
- [工程化缺口] storage.rs:278-281 — sync_all 仅 Unix，Windows 空操作但状态仍标 Verified/Committed。｜后续建议（Windows FlushFileBuffers 或文档明示跨崩溃保证范围）

### desktop 共享原语（8）
- [过度抽象] theme.rs:412-425/653-678/57-58 + choice_group.rs:107/120 — reveal/disclosure/banner_note/SIDEBAR/COVER/disabled builder 全仓零调用方。｜已修复 0669787（删除真零调用项；theme::disclosure 有实际调用方保留；disabled 系 builder 已文档化为预留能力）
- [抽象不足] theme.rs:719/settings_ui.rs:4155/preferences.rs:601 — 字号档位 [1.0,1.25,1.5,2.0] 三处硬编码。｜已修复 cebda60（FONT_SCALES）
- [抽象不足] views.rs:118/351/theme.rs:243 等 — 标题栏几何与面板宽度魔法数多处重复。｜设计任务处理（随布局改版提取命名常量）
- [抽象不足] service_test.rs:108-154 vs model_discovery.rs:281-310 — 两套单实现 Transport + 近乎逐行复制的 ureq bounded 管道。｜后续建议（抽共享 bounded-request 帮助函数）
- [抽象不足] theme.rs:132-140 — apply_preference 内联 ease_out 同式与 280ms 不复用 motion 词汇表。｜已修复 0669787（PALETTE_MS + motion::ease_out）
- [hack] backend.rs:81-95 — 脱敏用硬编码 JSON 路径抠引擎 Request 密钥，引擎改字段名则密钥静默进日志。｜后续建议（引擎导出 secrets() 访问器；跨 crate 契约改动需配合发布节奏）
- [抽象不足] views.rs:584/main.rs:1524/import_ui.rs:2807/settings_ui.rs 多处 — settings_tab 魔法下标散落四文件且有 ==5 越界钳制残留。｜部分修复 87a73c2（钳制收敛到 normalize/select；SettingsTab 枚举后续建议）
- [工程化缺口] backend.rs:306-360 — Environment::detect 用位置下标回填字段，重排即静默错配。｜已修复 ed97cbf（命名键值采集）

### native/构建/CI（4）
- [hack] justfile:37/package.py:91/homebrew 模板/Package.swift:6 — 最低系统版本三处口径不一（14.0 vs sequoia vs macOS v15）。｜已修复 62fe72a（统一 15.0）
- [工程化缺口] package.py:134-135/148 + release.yml:182-189 — 有签名身份但公证材料不全时静默跳过公证。｜已修复 62fe72a（显式报错）
- [抽象不足] desktop.yml:15-45 vs release.yml:128-200 — 桌面打包流水线复制粘贴且已漂移。｜后续建议（composite action/workflow_call；CI 结构调整）
- [工程化缺口] shim.swift:128-152 — 手工镜像上游 HF 仓库名与文件清单，上游改布局则缓存准备成功而加载失败。｜后续建议（CI 加「prepare 后离线加载」冒烟）

## 低严重度（51 条）

已修复：llm.rs:198-201 进度条漏计（c5a9958）；render.rs write_outputs 不走 atomic_write（feb89cc）；apple.rs vad() FFI 空指针（00452d8）；progress.rs Mutex unwrap 毒化（00452d8）；dispatch.rs Guard Drop 锁中毒（feb89cc）；pipeline.rs probe_duration 静默当 0（8689a9d，保留 run.json 诊断）；models.rs prepare 吞二次检查错误（04a8e5b）；wizard.rs:128 中途 exit(0)（04a8e5b）；NPU worker HTTP 无鉴权 token（4bfe373）；main.rs json_requested 近似（04a8e5b 注释）；llm.rs is_retryable 死路径（04a8e5b）；pick_subtitle_file 只认小写 .srt（04a8e5b）；motion enter/state_enter 死参数 _cx（0669787）；youtube 品牌色值与注释不符（保留实际渲染取值，注释已随主题检查核销）；model_diagnostics 写死 qwen3-1.7b（3a4e0c0）；provider==5 魔法数（a315e34）；settings_tab==5 渲染期钳制（87a73c2）；folder_editor 死删除确认块（随 H9 一并核销）；Drop 不取消 subtitle_cancel（cc62af5）；notes.rs manifest unwrap（cc62af5）；默认模型字面量分散（3a4e0c0）；reader_image_actions 恒等 reveal 闭包（0669787 核销）。

后续建议：asr.rs:1135 Energy::load 吞错（补 warn）；云端 ASR 并发写死 4（收进配置）；config.rs:466 tilde 展开 Windows；out_root 注释布局；doctor/wizard provider 标签两份（引擎侧）；两个文件名净化器合并；failure_message 按 " / " 截双语串（结构化字段）；#[path] 非常规模块布局；models.rs Api 分支两种防御风格；wizard/provider 引擎侧标签统一；advance_conversion_when_ready 抓第一个窗口（已有 library_ui 同款修复模式）；box_section 中文案 match 图标；stop_session 吞 set_intent 错误（聚合成一条 issue）；public_config 脱敏清单无编译期保障（serde 属性/集中清单）；apply_* 一行 setter；onboarding 死参数/兕底分支/Debug 格式 ID；步骤元数据两处真相；existing_source_note 每渲染遍历克隆全部课程（缓存）；onboarding 面板宽度公式与断点魔法数；task_component_failures 死参数；正文插图写死 16/9；request_layout expect（改 debug_assert）；service_configuration_issue 子串分类（worker 产出 error_kind）；source.rs probe 语言预排序不一致；notes.rs 扫描深度魔法 8（命名常量）；queue_task_card 每帧 clone（Rc 快照）；icons 双名别名收敛；性能开关环境变量解析不一致；spawn_input UI 线程写 stdin 契约文档；bench-mac.sh 精确 kill 与失败不计数；install.sh 校验和验证（release 发布 SHA256SUMS）。

## 处置总览

- **已修复（本目标内）**：14 项高危中的 12 项完全修复、2 项部分修复（H6 深改 schema 留后续、H5 随设计任务拆分）；中危 60 项中 38 项修复、6 项部分修复/移交设计任务；低危 51 项中 21 项修复。
- **移交设计/动画/信息展示任务**：设置页图标来源、reader 目录选中态、任务卡动作去重、service_editor 拆分、Textarea 固定高度、标题栏几何常量、模型下载进度统一、onboarding 阶段嗅探结构化。
- **后续建议（记录原因）**：dispatch 收据 O(N²) 索引、.part 断点续传、Windows sync_all、secrets() 跨 crate 访问器、Option<AsrProvider> 深改、CI 流水线去重、离线加载冒烟、结构化错误类别等——均非 rc 阻塞项。

## 总体观察（跨区域共识）

1. **取消契约未铺满是最集中的工程化缺口**：引擎的最长操作（llama-server 加载 300s、yt-dlp 下载、ffmpeg 切段、阻塞 HTTP 120-300s）几乎不轮询 control.json；desktop 侧也有不可取消的 probe/渲染期 IO。直接影响「任务暂停/取消」可信度与信息展示。
2. **渲染期副作用是 desktop 的系统性违规**（H3/H4/H7 + 多处渲染期状态改写），与 SKILL.md 明文约束冲突，也是设置页状态混乱（含 H13 选择回退）的温床。
3. **同一事实多份拷贝已开始漂移**：provider 编码/标签（4 处）、默认模型（9 处）、metallib 路径、NPU 路径、字号档位、标题栏几何、HTTP 管道、密钥脱敏路径。建议设立「后端探测与默认值」单一事实源模块。
4. **持久化/崩溃恢复的主路径质量很高**（checkpoint/artifact/preferences/workspace 均有行为级测试），但补做下载校验、render 输出、凭据损坏、journal 损坏等「第二路径」没获得同等待遇。
5. **测试文化两元化**：支撑层（choice_group/motion/backend/dispatch）有真实行为测试；同时存在 include_str! 源码哨兵测试与恒值函数+死分支的「设计钉桩」机制，后者应整体清除。
6. **没有架构性过度抽象**：单实现 trait 均服务于测试注入，合理；过度抽象集中在死 API/薄包装/恒值函数层面，适合一次清扫。
7. **信息展示的直接证据**：模型下载进度两处消费者+字符串过滤+1s 合帧（设置页明显滞后）、进度条 total 含跳过 chunk 停不满、字节格式化两套、补做路径把未校验文件标已校验——与用户感知的「进度条让人焦虑」吻合。

## 真机复现记录（macOS M3 Max，2026-09-14）

- CLI `course2md doctor`（2.0.0-rc.2 二进制）：正确识别 `MTL0: Apple M3 Max`，但同时把 `BLAS: Accelerate` 列为 GPU（H14）。
- 安装版 2.0.0（/Applications/course2md.app，bundle dev.course2md.desktop，pid 7302，cua-driver 截图取证）：
  - 设置→生成笔记→高级设置→本机引擎：GPU 选项可见可聚焦。
  - 点击 GPU：出现聚焦环，识别模型行短暂变为单选宽版，但 `generation.json` 从未写入 provider="gpu"（实证：rev 27 null → rev 28 coreml）。
  - 随后一次 cua-driver scroll（注入 PageDown/方向键）把已聚焦的选择组导航到 coreml 并提交落盘——用户视角为「选 GPU 没反应，过一会儿自己跳回 Apple 原生」。
  - 结论：「不识别 GPU」的表层症状 = GPU 可检测（doctor 证明）但**桌面端选择不持久 + 静默回退**；根因方向：设置渲染期的状态水合/提交链路（H4/H13 区域）与 choice_group 键盘导航提交语义。HEAD 复测后定位到具体代码。

## 后续修复记录（2026-09-15 凌晨）

在终审运行验证中发现并已修复的新问题（均不属于原审计表）：

1. **主线程同步 Keychain 读取可冻结整个应用（高危）** — `start_next_task → request_for → resolve_for_execution → vault.resolve` 在主线程同步读取 macOS 钥匙串；当 SecurityAgent 授权弹窗队列被其他应用占用时，整个界面（调度器、点击、渲染）无限期冻结。已改为先收集所需凭据引用、在专用 OS 线程解析后再回到 UI 线程启动任务（`start_pending` 守卫 + 启动前状态重校验）。
2. **GPUI background_executor 任务可能在主线程执行（高危）** — 采样冻结进程确认 `dispatch` 到 GCD 全局队列的 Runnable 经由 queue override 跑在 `com.apple.main-thread` 上；`cx.spawn` 更是始终在主线程轮询。凡是含阻塞系统调用（Keychain、同步 HTTP、子进程探测、数 GB 文件的 SHA-256、大目录迁移）的 executor 任务都有冻结界面的风险。新增 `spawn_blocking_io()`（专用 OS 线程 + smol channel），并将全部此类调用点迁移过去：任务凭据解析、服务模型发现（设置页与引导页）、服务连通测试（两处）、环境探测、笔记/阅读器加载、B 站账号状态与扫码轮询、字幕读取/刷新、课程库迁移与重新关联扫描。
3. **作业监视器 reader 线程无界 join（中危）** — 取消后若管道读取端未随子进程退出而关闭（孙进程继承写端），`t_read.join()` 永久阻塞，`Event::Exit` 永不发送，任务槽被占死（后续任务全部排队、卡片停在"正在停止…"）。已改为最多 3 秒有界等待后再发 `Event::Exit`；reader 线程在管道关闭后自行结束。
4. **install.sh `set -u` 在收尾提示处中断（低）** — `$BIN_DIR，` 的全角逗号被 bash 并入变量名，安装完成后报 unbound variable。已加花括号。
