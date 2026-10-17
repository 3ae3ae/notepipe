# 代码审查遗留建议

分支 `refactor/review-fixes` 已完成两轮修复。本文件记录第二轮独立复审后**仍未修复**的问题，供后续逐项处理；修完一项就删掉对应条目，全部修完后删除本文件。

审查标准（六类）：抽象是否恰当；hack；性能；其他坏味道；过度防御 / 过度测试 / 过度文档；用户文案是否过度说明、难懂或用词不一致。

用词约定：校对 / proofreading（不用润色）、摘要 / summary（不用总结）、笔记库（不用课程库）、文件夹、保留的视频、首次设置、AI 服务、语音识别 / 语音识别服务、登录状态，引号统一用「」。引擎与 CLI 文案保持「中文 / English」两半同义。

## 建议优先处理

1. **工作区保存在 UI 线程同步写盘**（`desktop/src/workspace.rs` `transaction`/`write`）：选任务、折叠卡片、切换笔记库、自动保存都会触发，笔记库在外置盘或网络盘时界面会卡住。改为后台合并写入，需落盘后才能继续的操作等待写入结果。
2. **按文案驱动逻辑仍有多处**：任务卡按报错文字决定「修复 / 重试」（`task_ui.rs` `service_configuration_issue`）、按 `" / "` 切中文（`summarize.rs` `failure_message`、`notes.rs` `chinese_half`）、按下载进度文字猜阶段（`activity.rs` + Apple shim 进度消息）、按校验报错原文清除状态（`main.rs` `source_validation`）。改为引擎输出结构化原因 / 阶段，桌面端按类型分支。
3. **同一概念多种编码**：识别引擎按 `PROVIDERS` 下标持久化（改表顺序会改掉用户选择）；`TaskRecord.outcomes` 为无类型 JSON；`folder_filter` 用 0 表示未分类；Apple 模型清单在 Swift 和 Rust 各写一份且已不一致（可能出现「显示已下载但无法离线加载」）。
4. **阻塞与重复计算**：启动任务时在 UI 线程把完整请求（含字幕）写入子进程 stdin；yt-dlp 只保留前 64KB 错误输出，长输出时丢失真正的错误行；异步流程里仍有整版 SHA-256，恢复路径对全部文件哈希两遍；任务列表、笔记列表、截图网格、正文查找有每帧克隆或重算。
5. **跨平台与导航**：阅读页查找快捷键绑定 `cmd-*`（Windows/Linux 无效）、「查看任务」绕过 `navigate`。
6. **用词统一**：润色/校对、总结/摘要、课程库/笔记库/存储位置、检查/测试、暂停/停止、接口类型/服务类型/协议仍混用；部分 CLI 帮助中英两半不一致。

## 未完成的验证

- 原生应用验证在中途因额度中断，没有得到验证结果：转换流程、阅读页、笔记库、设置各页、失败状态、窄/宽窗口、深色模式、其他配色与 150% 字号都需要在运行中的应用里重新检查（方法见 `desktop/README.md` 的原生 UX 检查）。
- `.claude/review/round2/wip-llm-provider.patch` 是未完成的 `llm.rs`/`provider.rs` 改动（把 `chat_body` 移入 provider.rs 等），当时编译不过，未提交。

## 详细清单

以下按区域列出，行号以本分支当前代码为准（重新定位时按内容查找）。

### Round-2 findings: engine core (src/asr, pipeline, llm, fetch, dispatch, execution, runtime, checkpoint, summarize, provider, artifact)

#### 中
1. fetch.rs:1001-1011 run_download keeps only the FIRST 64KB of yt-dlp stderr, but cmd_error takes the last 5 lines; with -v or many warnings the final ERROR: line is lost → wrong user message, is_bilibili_412 misses, deterministic errors retried 3x. Keep the LAST lines/64KB (ring buffer).
2. summarize.rs:184-195 failure_message splits on " / " to keep only Chinese → text-driven, inconsistent with other bilingual Outcome messages, truncates URLs. Store the full bilingual message; let the desktop choose what to display (desktop already has notes::chinese_half).
3. pipeline.rs:416 "没有提取到截图 / No screenshots captured" vs :827 "未提取到截图 / No screenshots were captured"; desktop/src/activity.rs:255-256 matches both strings. Use one shared constant (中文「未提取到截图」), desktop references it — better: make the desktop not match text at all if a structured signal exists.
4. 润色 vs 校对 terminology mixed. llm.rs:916 "  LLM 润色 / Transcript polish: {}" → "  AI 校对 / AI proofreading: {}"; :939 "  截图辅助润色 / Slide-assisted polish" → "  截图辅助校对 / Slide-assisted proofreading"; :946 "开启润色 / Enable transcript polish: course2md llm setup" → "开启 AI 校对 / Enable AI proofreading: course2md llm setup"; :886 "润色时附上幻灯片截图？… Attach slide images when polishing?…" → "校对时附上截图？需要支持图片的模型。 / Attach slide images when proofreading? Requires an image-capable model."; pipeline.rs:943 "可开启 AI 润色与总结" → "可开启 AI 校对与摘要"; sync src/cli.rs help and any other "润色/总结" user text.
5. fetch.rs:917 BILIBILI_412_HINT tells desktop users to run "course2md --login bilibili" (shown via desktop/src/source.rs:251); auth::with_bilibili_login_tip likewise appears in desktop task errors. CLI hint only on the CLI; the desktop shows "在设置中重新登录 B 站" style guidance.
6. artifact.rs:319-322 publish checks Handle::try_current().is_err() to run synchronously "for desktop unit tests"; desktop calls smol::block_on(artifact::publish(..)) in 5 places. Remove the runtime check; desktop calls publish_blocking directly.
7. Blocking IO in async fns contradicting pipeline.rs:1010 comment: run_prepared (:683) and reprocess (:271, :296) call artifact::published / validate_version (full SHA-256); reprocess :273 digest, :362-373 image copies, :433 timeline read; export_only sync render+digest. Resume path hashes everything twice (run_prepared then publish_blocking under lock). Wrap in blocking(...); when manifest already validated, publish does a light identity check.
8. pipeline.rs:322-325 starts_with("snapshot:") vs dispatch.rs:398 format!("snapshot:{}") → shared const/fn in dispatch.
9. execution.rs:23, :59, :247, pipeline.rs:200 parse Component from Vec<String> repeatedly → derive Serialize/Deserialize (rename_all lowercase), use Vec<Component>, delete as_str/parse/parse_all. Update desktop callers (wire format unchanged).
10. llm.rs:156 chat_body only called by provider::chat_body → move into provider.rs, make CHAT_TEMPERATURE private.
11. llm.rs:17-22 and :454-456 output-format contract stated twice in one system prompt with different wording → drop the sentence from DEFAULT_PROMPT, keep the appended one, use 「校对后的文本」.

#### 低
- asr.rs:371 run_chunks callback FnMut(usize, Seg, &Path) — all callers ignore first two → FnMut(&Path).
- asr.rs:252-256, :462-466, npu.rs:177, apple.rs:334: "empty after VAD → warn 未检测到语音 → return empty" repeated in 4 backends → fold into run_chunks or helper.
- asr.rs:1227 `if dur > 0.0 {..} else { e + PAD }` else unreachable → remove.
- asr.rs:135-137 magic 9 = "language ".len() → strip_prefix.
- asr.rs:296, :874 LAST_LLAMA_SPAWN_ARGS global side channel reset by pipeline → attach spawn args as error context instead.
- dispatch.rs:514 `status == 429 || (400..500)` redundant.
- dispatch.rs:635, :641 `32 * 1024 * 1024` twice; llm.rs:802 `4 * 1024 * 1024` → named consts.
- dispatch.rs:580-603 no-ledger CLI path re-implements 4xx/429/5xx classification of send_described → shared classify(status, body).
- pipeline.rs:451-461 and :872-882 build-sections logic duplicated → build_sections(frames, speech, duration).
- pipeline.rs:807, :836, :1101 `events.iter().any(|e| !e.text.trim().is_empty())` ×3; artifact.rs has has_readable_body → one predicate.
- pipeline.rs:1242 write_failure_run_json 6 params duplicate FailureRecord fields → method.
- pipeline.rs:150 run_task re-validates request already validated by execution::run → remove one.
- summarize.rs:267-304 map step waits per batch of 4 (not a worker pool) → reuse llm.rs run_pool pattern.
- summarize.rs:243 llm::validate duplicated with callers (check main.rs summarize path before removing).
- llm.rs:246-254 vision mode loads ALL screenshot base64 up front and keeps it → load per batch.
- fetch.rs:1013 "等待 yt-dlp 结束失败" not bilingual → "等待 yt-dlp 结束失败 / Failed waiting for yt-dlp to exit"; :376, :378, :458 debug logs too.
- fetch.rs:805-891 and :925-955 two stacked 412 retry loops, outer classifies by format!("{e:#}") text → one helper with typed error.
- fetch.rs:429 and desktop/src/source.rs:421 comment refers to nonexistent "interaction.md" → remove reference.
- artifact.rs:362, :427, llm.rs:27, :184 history comments ("原实现逐张串行", "原先各复制一份", "取代散落的字符串比较") → delete.
- checkpoint.rs:566 "B2 回归" review id; :3 "v2 语义（1.0 起）", :183 "1.0 前旧 checkpoint" version history → trim.
- checkpoint.rs:290 pub fn events() test-only → cfg(test) or tests use sorted_events.
- runtime.rs:325-326 comment says dir name has tag and pid; code has no pid → fix comment.
- runtime.rs:209, :215 halves disagree → "{0} 启动后意外退出（{st}） / {0} exited during startup ({st})"; "{0} 启动超时（{1:.0} 秒） / {0} startup timed out ({1:.0}s)".
- execution.rs:108 hardcodes "16 MiB" separately from MAX_REQUEST_BYTES → format from const.
- provider.rs:125-141 has_structured_format / strip_structured_format each match provider for field name → structured_format_key(s).
- Tests asserting error wording: asr.rs:1337, summarize.rs:589, fetch.rs:1310 → assert type/behavior.

### Round-2 findings: engine support / CLI / tests / build / native shim


#### 中
1. Apple model repo ids + required files hard-coded twice and already inconsistent: native/apple-asr/Sources/CAppleASR/shim.swift:135-146 vs src/model_status.rs:143-201 (whisper: shim downloads manifest.json/vocab.json/merges.txt, model_status doesn't check them; qwen3-1.7b: model_status requires config.json, shim doesn't list it). One table in Rust, passed to c2m_asr_prepare_cache over FFI.
2. shim.swift:150 progress message is the HF repo id (aufklarer/Qwen3-ASR-CoreML) passed through src/apple.rs:57-65; desktop/src/activity.rs:321-344 guesses the phase by substring ("silero/whisper/qwen", "加载"). Add a structured phase (vad/asr/load) in the callback → progress events carry `phase`; neutral message; desktop uses phase.
3. src/main.rs:149-151 models prepare emits a fake done event ("slides":0,"segments":0, title = model) with a wrong comment; :159-163 models download emits a different done shape. Define a model-ready event (e.g. {"type":"model_ready", …}) or an operation-tagged Completed; update desktop/src/backend.rs and its consumer.
4. Tri-state flag merge written 4 ways: config.rs:603-611 resolve_resume and :621-629 resolve_mmproj_offload (identical), options.rs:114 keep_video inline, options.rs:11-44 llm/vision/summarize inline; config.rs:614-616 resolve_gpu_layers is just cli.or(file).unwrap_or and its test (config.rs:973-986) tests Option::or. → one fn flag(on, off, file, default); delete resolve_gpu_layers + test.
5. src/timeline.rs:236-239 latin_word only ASCII + Latin ranges → Cyrillic/Greek words glued ("Привет"+"мир"). Use is_alphanumeric() excluding CJK/kana/hangul; add a Russian test.
6. src/login/codex.rs:418-487 browser auth polls accept with sleep(200ms) for up to 180s, called by authorize_for_desktop (:502) with no cancel → accept a cancel flag checked in the loop; desktop passes it (cancel when user leaves/cancels).
7. 摘要 vs 总结 mixed (see decision above): main.rs:419, :463, :500, :495; cli.rs:235, 293; desktop has both ~46/48 times. Unify to 摘要.

#### 低
Abstraction/dup:
- wizard.rs:239-248 configure_cloud re-implements config::ensure_http_url (English-only parse error) → use it.
- subtitle.rs:507-522 lang_rank parallel to sort_tracks → pick_subtitle_file builds tracks and calls sort_tracks(…, "zh", Some("en")) like sidecar_subtitle (:526).
- config.rs:176-188 model_label lists all models again → look up local_models(AsrProvider::Npu) (contains all).
- apple.rs:189-193 note text by id match → note field in LocalModel.
- apple.rs:285, 327 VAD params 0.25, 0.35 twice → consts.
- apple.rs:262-287 writes a 1-second silent WAV to verify VAD loads → add c2m_vad_load in the shim.
- shim.swift: two error channels (errBuf vs c2m_last_error) → one.
- models.rs:35-77 fetch_model cached/verify params unused by Cpu/Gpu/Npu → push down or split.
- media.rs:157-160 probe_duration_blocking duplicates parse_duration → reuse.
- portable.rs:104-109 mime→ext separate from image_mime → image_mime returns (mime, ext).
- login/codex.rs:576-587 model-catalog fallback duplicates desktop_catalog (:88-102); fetch_models (:36-45) re-writes auth_headers → reuse.
- build.rs:172-216 xcode-select -p run twice; find_clang_rt_osx tuple first item unused → one call, return dir.
Hacks/hardcoding:
- config.rs:361-376 Roi::pixels "≤1.0 = fraction, >1.0 = pixels" (pixel 1 = 100%) → typed unit or percent only (help documents percent).
- config.rs:739-753 youtube_id split_once("v=") matches nav=... → host check + query_pairs.
- config.rs:639 default_provider_hint uses require_cmd (builds an error) → runtime::which(..).is_none().
- npu.rs:66 Python picks pipeline by "qwen" in model_arg → pass explicit kind.
- npu.rs:47-51, 92, 106 dead script defaults, /v1/health, /v1/shutdown, /exit aliases; :57, 64, 85, 139 prints to stdout which is Stdio::null (:353) → delete.
- npu.rs:335 only python3 vs doctor.rs:119-121 accepts python → one rule.
- auth.rs:30 message.contains("--login bilibili") to avoid duplicate tip → marker error type + downcast.
- settings.rs:256-266 template hard-codes concurrency = 8, deepseek-chat; :210, 225, 239, 241 bool/enum defaults and :237 Unix path hard-coded, contradicting :160 comment → render from Default values.
- settings.rs:303 {:?} + to_ascii_lowercase to display TranscriptSource → Display impl.
- build.rs:145 OUT_DIR/../../../ relies on cargo layout → comment the limitation or copy in packaging.
Dead/defensive/naming:
- progress.rs:374-389 "scenes"/"asr"/"summarize" branches never hit; protocol doc (:3-8) misses the workers event.
- Stage names inconsistent: npu.rs:254 model-load, apple.rs:343 model/apple, models.rs:110 model/prepare → model/… prefix (update desktop consumer).
- model_status.rs:113-128, 193, 240 unreachable branches after canonical_model.
- scene.rs:332-335 re-validates sample_interval ≥0.2 (validate did); :441 candidate_last_t.is_some() always true.
- scene.rs:516-528 continues extracting all frames after one fails → return on first Err.
- wizard.rs:15 `!config_path().is_file() ||` redundant; :11 is_first_run pub but module-only, awkward bool param.
- wizard.rs:186-192 "退出" in no-backend menu returns an error (exit 1) vs :129 ExitWizard (exit 0) → ExitWizard.
- login/codex.rs:622-637 LoginStatus::Connected(String) always "Codex" → unit variant.
- models.rs:409 manifest write error swallowed with let _ → warn.
- timeline.rs:19 comment "未润色时与 raw 相同" wrong (raw is None) → fix.
Comments/tests:
- History/review comments: config.rs:84, :277, :416-420; render.rs:73; npu.rs:364-365; build.rs:33-38; timeline.rs:73 refers to nonexistent docs/DESIGN.md §3.2; build.rs:306 --short=12 mismatches code.
- Tests of constants/wording/library: wizard.rs:260-264 (Option::ok_or), models.rs:530-540, progress.rs:624-632, auth.rs:518-529 (QR code table), render.rs:250-254 (historic wording), tests/login_cli.rs:65 (asserts hint text).
- tests/task_execution.rs:206 sleep(150ms) then assert absence → explicit marker / after-the-fact assert.
- subtitle.rs:749-779, tests/scene_synthetic.rs:480, 519 temp_dir()+pid dirs leak on failure → tempfile::tempdir().
Text (bilingual halves must match, AGENTS.md):
- cli.rs:84 → 下载视频最大高度（推荐 1080）/ Maximum download height (1080 recommended)
- cli.rs:109 → 识别后端：coreml（Apple 芯片）、gpu、cpu、npu（Intel）、api（云端）/ Speech backend: coreml (Apple Silicon), gpu, cpu, npu (Intel), api (cloud)
- cli.rs:125 → 云端识别密钥；建议改用 COURSE2MD_ASR_API_KEY 环境变量，避免留在命令历史 / Cloud API key; prefer COURSE2MD_ASR_API_KEY to keep it out of shell history
- cli.rs:170 → 本地模型目录（默认使用系统缓存）/ Local model directory (default: platform cache)
- cli.rs:205 → AI 校对密钥（建议用 llm setup 设置，避免留在命令历史）/ AI proofreading key (prefer llm setup to keep it out of shell history)
- cli.rs:235 → 本次生成视频摘要并写入笔记 / Generate a video summary and add it to the notes
- login/codex.rs:269, 275, 286, 608, 614 登录态 → 登录状态; 润色 → 校对
- npu.rs:44, 61, 74 Python errors Chinese-only (shown via stderr tail) → bilingual.
- scene.rs:380, 497 progress templates English-only → 扫描 / sample {pos}/{len}, 截图 / extract {pos}/{len}.
- doctor.rs:78 「随包版本」 jargon + hard-coded install command duplicating install_hint → use install_hint("yt-dlp"); 「重新运行 install.sh 可安装内置版本」.

### Round-2 findings: desktop shared primitives + reader

#### 中
1. Display text drives logic:
   - main.rs:468 `source_validation.as_deref() == Some("请填写笔记名称")` matches task_ui.rs:1121 ensure! text → enum SourceValidation.
   - "timestamp-only heading" detected by comparing heading text to fmt_ts(seconds) in reader_navigation.rs:216, reader_ui.rs:1366-1367, reader_ui.rs:4325-4326 (root notes.rs:340-344) → PreviewBlock::Heading { timestamp_only } or separate variant.
   - notes.rs:245-248 chinese_half splits engine text on " / " (breaks on "YouTube / Bilibili"); reader_ui.rs:2127, 2320 apply it to the desktop's own Chinese errors → engine stores structured zh/en (or the full message is stored and the desktop shows it as-is).
2. Parallel encodings:
   - Engine choice persisted as PROVIDERS index usize (main.rs:70-77, 134, 110-123; workspace.rs:118 local_provider: Option<usize>), onboarding uses Option<AsrProvider>; CLOUD_PROVIDER_INDEX = 5 magic; asr_provider_from_index matches strings. Reordering the table silently changes saved choices → ConversionOptions.provider: Option<AsrProvider> serialized via as_str, with a deserializer accepting the old numeric index; delete CLOUD_PROVIDER_INDEX/asr_provider_from_index; PROVIDERS only labels.
   - Reading-position key: main.rs:1300-1301 hand-written format!("{}:{}:{tab}") vs reader_ui.rs:629 manifest_reading_key; reader_ui.rs:1557 another format → one function.
   - Summary blocks recognized by anchor strings ("summary", "summary-tldr", "key-point-") reader_ui.rs:717-725; "section-{n}" reader_ui.rs:5034; depend on notes.rs:307-355 format! → PreviewBlock role / section_index.
   - Reprocess components as strings "screenshots"/"proofreading"/"summary" (reader_ui.rs:3806-3812) parallel to notes::ProcessingStage → use the engine Component enum.
3. Polling as synchronization: main.rs:507-519 100/500ms poll drives task events, draft debounce (350ms effectively up to 500ms), banner expiry, quit timeout; 8s expiry duplicated main.rs:865 and views.rs:195 → job events via async recv, timers per concern, one constant.
4. reader_ui.rs:3855-3862 "查看任务" sets this.page = Page::Task bypassing navigate (read_generation, reading=false, message/temp cleanup) → navigate(Page::Task, cx).
5. reader_ui.rs:501-503 binds cmd-f / cmd-g / cmd-shift-g (Super on Win/Linux); main.rs uses secondary-*; reader_ui.rs:3229 hint hard-codes "⌘F" → secondary-*, platform hint.
6. Per-frame work:
   - reader_ui.rs:4114-4117 + 4234-4246 gallery not virtualized; each render calls frame_excerpt and nav::text_matches per card (lowercase + map of all transcript) → compute excerpts/highlights in update_gallery_matches, cache Rc.
   - reader_ui.rs:2169-2190 each keystroke calls text_matches for every block lowercasing/allocating → precompute lowercase text+map per block at load (like Frame.search_text).
7. Shared primitives bypassed:
   - theme.rs:528-529 comment says section_title shared with workbench recent notes, but course_library.rs:75-77, 2415 use own 24px SECTION_TITLE → use shared primitive (or theme token) and fix comment.
   - Warning boxes hand-built 3x: reader_ui.rs:3751-3757 (border same as bg), 3944-3952, views.rs:457-469 (DANGER) → NoticeKind::Warning + notice_banner.
   - `.text_sm()` (0.875rem) 41x in 9 files alongside TEXT_AUX (0.857rem); reader_ui.rs:1429, 1454, 3766, 3899, 3907, 3933, 3939 → TEXT_AUX everywhere.
   - Icon sizes px(18/20/22) (appearance_ui.rs:206, 375, 403; import_ui.rs 7×; settings_ui.rs 5×), rems(18/14), size_4() — px doesn't scale with text → theme ICON_SM / ICON_MD in rems, used everywhere.
   - appearance_ui.rs:393-403 overrides h(px(56.)), rounded, text_size(rems(16./14.)) after .settings_text() → shared outlined-select primitive.
   - State-layer opacities 0.035/0.075 (choice_group.rs:194-195) vs 0.08/0.10 (theme.rs custom_fill 792-793) → theme constants.
8. Text: reader_ui.rs:2993 "工作台" → "生成笔记" (page title views.rs:301); reader_ui.rs:2996 "我的笔记" → "笔记库"; views.rs:126 "course2md 工作台" → "course2md，生成笔记".

#### 低
- reader_ui.rs:2091 identity.starts_with("local:sha256:"); reader_ui.rs:3021, 2028 meta.extractor == "local" → identity-kind method.
- reader_ui.rs:3547 stamp.get(..10) slices formatted time → date-only formatter.
- "…意外中断，请重试" written separately at main.rs:1180, reader_ui.rs:2099, 2265, 2427 though main.rs:80 has INTERRUPTED → use constant.
- choice_group.rs:389-440 Tab and Radio branches near-identical; only Radio dims when disabled → shared builder, consistent disabled look.
- choice_group.rs:242-245, 288 scale × hand px (36/40, +4., +2.) → rems constants.
- focus_scroll.rs:59-60 FocusRing uses INK, others ACCENT.
- theme.rs:1029-1032 badge uses px(6/24/8/3), only non-scaling control.
- theme.rs:1089-1093 apply_scale re-validates FONT_SCALES already validated in preferences.rs:732.
- Compact 16px gutter in views.rs:33 and views.rs:538; settings_sidebar_width / settings_column_gap repeat 960/1200 breakpoints (960 = is_compact).
- performance.rs:362 writes file on background executor (should be spawn_blocking_io); performance.rs:315 comment says buffer allocated off UI thread but MotionTrace::new runs in start() on UI thread.
- reader_ui.rs:2040 is_file() in click handler (UI-thread stat).
- main.rs:407 `let message = None;` pointless; main.rs:417-426 `fields` third column always String::new().
- main.rs:246 folder_filter 0 = unfiled sentinel; main.rs:625, 629 f32::NAN initial values → enum/Option.
- reader_ui.rs:2280 history comment "（渲染不再负责）".
- Tests: reader_ui.rs:5718-5774 NoteListHarness reimplements wiring ("The same list wiring the reader uses"); motion.rs:587-630 three disclosure tests test GPUI layout; reader_navigation.rs:393-422 mostly tests chrono.
- Text: appearance_ui.rs:391 field legend "主题" duplicates group title and 浅色/深色主题 labels → delete; main.rs:76 "云端服务" shows as "识别设备：云端服务" (reader_ui.rs:3584) → "云端语音识别" or hide row for cloud.

### Round-2 findings: desktop import / task / workspace / library / storage / source / backend


#### 高
1. workspace.rs:1831-1927 transaction/write: every save, on the UI thread, clones the whole State, locks, runs check_library (reads marker) + ensure_work_directory per changed task, reads/compares/atomically writes mirrors, serializes the full record. Triggered by pure UI actions: task_ui.rs:1756 select_task, 2634 collapse card, course_library.rs:1430 group toggle, import_ui.rs:2663 switch library, every save_current_draft autosave. Freezes on external/network disks (contradicts storage_ui.rs:1033 rationale). → Background writer that coalesces saves with in-memory state authoritative; operations that must be durable before proceeding (enqueue etc.) await the write result; pure UI state saved debounced.

#### 中
2. task_ui.rs:360-392 service_configuration_issue substring-matches English keywords (api key, http 401…) and the engine's own Chinese text ("服务未找到此任务指定的接口或模型", "服务拒绝了请求参数") → engine emits a structured failure kind (auth/model/params) in Outcome/Blocked; desktop branches on it.
3. More text inference: task_ui.rs:320-345 task_failure_summary splits " / ", checks CJK presence, truncates 140; activity.rs:222-263 component_failure_message hard-codes legacy Chinese phrases; import_ui.rs:121-124 readable_first_line looks for WARNING:/ERROR:; source.rs:155 is_login_failure re-classifies stored display text (called import_ui.rs:628/1587/1612/2204) though preview_command_error already computed FailureKind; activity.rs:317-345 sniffs worker output for phase keywords. → store FailureKind with preview_error / SubtitleEvidence::Failed; new tasks store structured reasons; legacy compatibility isolated in one migration function. (Engine progress phase is being added by the engine pass — use it.)
4. workspace.rs:368 TaskRecord.outcomes is untyped serde_json::Value while engine has artifact::Outcomes; string-key access in workspace.rs:443-463, 941-1027, 1164-1169, 1246-1263, task_ui.rs:303-314, 398-408, 3710-3762; task_component_failures clones+deserializes JSON every render (course_library.rs:2286, import_ui.rs:2882 per frame) → Option<artifact::Outcomes>, same wire format.
5. task_ui.rs:1631-1688 finish_task_start on UI thread: request_for calls check_library and reads task_subtitle_events (≤32MB); Job::start_task spawn_input (backend.rs:129-134) synchronously write_all's the whole request (with subtitle events) into child stdin — blocks UI when pipe (~64KB) fills; finish_task read_manifest on UI thread (task_ui.rs:2119-2121) → whole start sequence via spawn_blocking_io (at least subtitle read + stdin write on a thread).
6. Per-frame recomputation: task_ui.rs:2307 queue_page clones workspace.state.tasks each frame (sort indices instead); course_library.rs:2271-2289 recent_notes_section .cloned() tasks per frame; import_ui.rs:3241 conversion_recovery calls preview_plan() per frame (cache on draft/preferences change); course_library.rs:984-1008, 1084-1092, 1732-1741 filtering calls course_location/course_folder 3-4× per note, each recomputing storage_dir() PathBuf + BTreeMap lookup + linear library scan, plus to_lowercase on all titles per frame; task_ui.rs:3281-3294 task_cover scans courses per card (tasks×notes) → precompute per-note library/folder, lowercase title, task cover index on refresh.
7. Two sources for retry labels: task_ui.rs:426-444 retry_component_label vs task_ui.rs:2972 format!("仅补{label}") → one function. Wording: 补生成截图 → 重新生成截图; 补生成摘要 → 重新生成摘要; 仅补导出 → 重新导出.
8. 0 = unfiled sentinel (main.rs:246; library_ui.rs:324, 617; course_library.rs:38, 1004, 1089, 1545, 1673, 1736) → enum FolderFilter { All, Unfiled, Folder(u64) }.
9. 课程库 in UI text: workspace.rs:1111, 1177, 1586, 1937, 1960, 1963; task_ui.rs:1053, 1453:
   - 保存位置对应其他课程库，请恢复原位置或在存储设置中重新登记 → 这个文件夹属于另一个笔记库，请在设置的存储中重新定位
   - 请选择已有文件夹；新建课程库请使用新建操作 → 请选择已有文件夹
   - 这个库已经登记在另一个位置，请使用移动课程库操作 → 这个笔记库已在另一个位置添加，请使用「移动笔记库…」
   - 课程库身份记录为空… → 笔记库身份记录为空，请检查所选位置
   - 任务工作目录与记录的课程库不匹配 / 任务材料位置越出课程库… / 任务位置与课程库记录不匹配… / 保存位置已不在课程库中… → 课程库 → 笔记库

#### 低
10. Dead code: library_ui.rs:743-794 folder_picker course/index params (only caller import_ui.rs:2695 passes (None, 0)) → remove with has_course branches; source.rs:26, 462, 673 Source.cover_error written never read → remove; import_ui.rs:1968-1971 match arms identical; stage aliases "summarize"/"export"/"exports" never emitted (activity.rs:277, 279, 305-306; task_ui.rs:267, 306); backend.rs:572-574 vertical_scrollbar forwards to _for; backend.rs:398-401 Environment.npu duplicates npu_device && npu_runtime.
11. library_ui.rs:53-322 holds source-reading logic (inspect_source, invalidate_source, start_source_cover_fetch) → move to import_ui.rs; library_ui.rs:176-235 closure too long for rustfmt → named methods.
12. Subtitle ordering twice with different fallbacks: source.rs:452, 731, 782 ("zh", empty prefs) and library_ui.rs:188, import_ui.rs:896 (user prefs, "zh-Hans") → only the UI-side one.
13. storage_ui.rs:1235 format!("{error:#}。保存位置尚未切换，已复制的内容保留在目标位置。") → double 。 and duplicated "尚未切换" → don't append when already stated; no extra 。.
14. Inconsistent wording: 清除搜索 (course_library.rs:1315) vs 清空搜索 (1366) → 清除搜索; 继续生成/继续任务 (task_ui.rs:2932-2934) vs 继续处理 (course_library.rs:2512) → 继续生成; 识别视频声音 (import_ui.rs:1973, 2112), 将识别视频声音 (1793), 识别服务 (import_ui.rs:2371, task_ui.rs:3333), 音频会发送到所选识别服务 (2433), 设置语音服务 (3388) → 语音识别 / 语音识别服务 / 设置语音识别服务; 重试保存生成选项 (import_ui.rs:3295) → 重试保存处理选项; 请重试保存输入记录 (task_ui.rs:1090) → 请重试; 请在设置的应用与诊断中查看安装方法 (task_ui.rs:3666) → 请在设置的「应用」中查看安装方法; mixed “” vs 「」 (storage_ui.rs:154, 1809; workspace.rs:474) → 「」.
15. History comments: course_library.rs:1471-1473, 1494; storage.rs:322; activity.rs:312, 360-361; task_ui.rs:454; task_ui.rs:1208 outdated "source_mode 1"; import_ui.rs:1271-1276 doc+inline say the same thing.
16. Tests: workspace.rs:1986-2002 accept_source test-only parallel implementation → test the real path; activity.rs:403 asserts !contains("约剩") (always true) → assert 预计剩余.
17. Misc: task_ui.rs:1840-1842 unknown purpose → unwrap_or(Component::Proofreading) silently → filter/err; storage.rs:437-447 read_records writes journal during a read and swallows errors with let _; import_ui.rs:3266 checking_components inferred from submission error → environment.is_none(); three-field reset duplicated import_ui.rs:2342-2366, library_ui.rs:396-398/557-559/514-516; storage_ui.rs:965 poll_storage and backend.rs:174/180 timer polling → event-driven (job exit, join results).

### Round-2 findings: desktop settings/onboarding/services/accounts + docs/tooling

#### 中
1. Model cache state labels in 4 inconsistent sets: model_diagnostics.rs:670-680, 1197-1211; onboarding.rs:1184-1196, 1699-1706 → one cache_state_label(CacheState) -> (BadgeKind, &str): 已就绪 / 已下载 / 未下载 / 未下载完整 / 不支持.
2. Render-time side effects: onboarding.rs:1166, 1672 (and import_ui.rs:2956) call ensure_model_diagnostic from render (writes pending_checks + cx.defer), contradicting settings_ui.rs:1100 / onboarding.rs:848 comments; model_diagnostics.rs:279 admits callers are render → trigger checks from event entries (setup_step, apply_setup_provider_choice, select_setup_model, page entry, environment refresh) and delete the pending_checks mechanism.
3. HTTP status classification duplicated with opposite results: service_test.rs:415-423 (429 = ContractMismatch) vs 245-251 Codex (429 = OutcomeUnknown); outcome and message are parallel if-chains (415-435) → one classify_status(status, code) -> (TestOutcome, &str).
4. Test-only parallel implementations: preferences.rs:1428-1442 resolve_for_execution and 1496-1515 record_draft_test are #[cfg(test)] while production uses credential_references_for + resolve_for_execution_with_secrets and service_editor.rs:1359-1367 fingerprint comparison → tests use production paths; delete record_draft_test; credentials.rs:370-397 credential_rotation test tests the MemoryCredentialVault double → delete/rewrite.
5. 课程库 / 存储位置 terminology in settings_ui.rs:2011, 2022, 2040, 2049, 2055, 2057, 2065, 2072:
   "添加存储位置" → "添加笔记库位置"; "课程库" → "笔记库"; "「{}」已登记，可在该课程库中“重新定位…”" → "「{}」已在笔记库列表中，可用「重新定位…」更换位置"; "课程库记录暂时不可用" → "笔记库列表暂时不可用"; "这个存储位置已经在课程库中" → "这个文件夹已是笔记库位置"; "已添加存储位置" → "已添加笔记库位置"; "存储位置尚未添加：…" → "笔记库位置尚未添加：…"; "无法选择存储位置：…" → "无法选择文件夹：…".
6. AI toggle / test names differ in 4 places: generation_settings.rs:291, 302, 313 (校对文字和标点 / 在开头生成摘要 / 校对时附带对应截图); onboarding.rs:1448, 1454 (自动校对文字 / 生成课程摘要); service_test.rs:34-37 (AI 校对 / 生成摘要 / 截图辅助校对); service_editor.rs:719-724 (语音 / 校对 / 摘要 / 截图校对) → onboarding uses the settings labels; TestKind::short_label() replaces the local match; "截图辅助校对" → "校对时附带截图". Ideally one shared label table.
7. account_ui.rs:310-312 comment claims account_status_text is the single source but only source_account_row uses it; account_summary (222-250) re-derives badge/detail; "退出登录尚未完成，原登录状态已保留" 3× (239, 318, 404); retry condition duplicated (420-424, 385-388) → state enum rendered by both; constant; method.
8. Two "recovery" dirs: preferences.rs:1601 root.with_extension("recovery") vs 1764 root.join("recovery") → one location.

#### 低
- Subtitle language table 3× with "custom" sentinel: generation_settings.rs:18-22, 77-84, 88-94 → one const table + enum for custom.
- Logic by display/id strings: settings_ui.rs:2418 `*name != "llama-server"`; model_diagnostics.rs:1103-1108 id strings pick rows; onboarding.rs:1476 id == "proofread" → typed field/flag in the tuple.
- onboarding.rs:99-100 fingerprint("configuration") sentinel contract → ServiceConfiguration::same_connection(&other).
- `match purpose { Speech => refs.asr, Ai => refs.llm }` ~6× (service_editor.rs:45-49, 61-65, 589-592, 1595-1598; settings_ui.rs:1416-1419; codex_ui.rs:300-304) → ServiceRefs::for_purpose.
- preferences.rs: dangling-default cleanup duplicated 1006-1021 and 1299-1314 → ServicesState::drop_dangling_defaults(); check_dispatch (1323-1342) vs preview closure (1369-1385) error branches duplicated; config_for_refs_with_dispatch_check 1389-1390 duplicates ServiceRefs::required_for (571-572).
- [Proofread, Summary, Vision] repeated (service_editor.rs:1069, 1080, 1408, 1425; settings_ui.rs:1435); "proofreading"/"summary" magic strings → TestKind::AI const + engine Component.
- settings_ui.rs:2097-2100 hand-written 100/125/150/200 parallel to FONT_SCALES and each option has the same icon; generation_settings.rs:596, onboarding.rs:1220 same mic icon on every model option → drop decorative icons on values; derive from FONT_SCALES.
- service_editor.rs:1549-1563 close_service_editor always returns true → branch at 559-565 dead; return ().
- service_editor.rs:398 `&& has_services` always true in else of !has_services.
- ollama_ui.rs:42 generation check can't fail.
- codex_ui.rs:177 writes placeholder LoginStatus::Connected("Codex") then re-reads → keep None until read returns (note: engine pass may make Connected a unit variant).
- preferences.rs:1740-1750 atomic_write_with_retry retries 3× with no delay → remove or justify.
- preferences.rs:617-650 StoredTestEvidence parses "HTTP n" from details text → remove the shim (old records simply lack http_status).
- Thin wrappers: settings_ui.rs:183 (text), 623 (group), service_editor.rs:33, 1384, model_diagnostics.rs:101, 1161; settings_ui.rs:224 settings_form_row dispatches two layouts on a bool → two functions.
- ollama_ui.rs:165 push then insert(0, …); Ollama recheck icon-only vs Codex labeled button though comment says same composition.
- credentials.rs:128, 186 service-name constant duplicated; "请输入 API Key" empty check in 3 vault impls.
- UI-thread file IO (small): account_ui.rs:396 clear_bilibili_login(); account_ui.rs:533 login.save(); service_editor.rs:1348, onboarding.rs:612 record_test; settings_ui.rs:2024-2032 canonicalize + read_to_string → spawn_blocking_io.
- Comments: ollama_ui.rs:136 "(review#1)"; settings_ui.rs:2478 "Wrappers only while … being integrated."; settings_ui.rs:1 "Four independently saved setting groups" (actually 3 PreferenceGroup, 5 tabs); model_diagnostics.rs:1177 "stays behind 技术详情" (button is 管理模型与下载); preferences.rs:994 legacy tombstones history.
- Text:
  - settings_ui.rs:1708 success shows "尚未应用" with a green check → "已暂存，点击「应用」后生效"
  - settings_ui.rs:1368 heading "添加语音服务" duplicates the button when services exist → drop heading when non-empty
  - settings_ui.rs:2159 label "首次设置" + button "重新运行首次设置" → button "重新运行"
  - settings_ui.rs:2263 "重新检查" vs 2377 "重新检查环境" → "重新检查"; 2299 "诊断详情" vs 2391 "查看诊断详情" → "查看诊断详情"
  - settings_ui.rs:1993 "保留原文件并重置此组" → "保留原文件并重置这些设置"; 2584 "原文件已保留，此组设置已重置" → "原文件已保留，这些设置已重置"; preferences.rs:1591 → "这些设置暂时无法读取，原文件已保留"
  - service_editor.rs:1706 "…相关后续请求会停止派发" → "…这些任务中用到此服务的步骤将不再继续"; 1738 "已停止派发，删除记录尚未完整保存" → "已停止使用此服务，但删除尚未完整保存"
  - preferences.rs:1207 "…原任务不会自动恢复外发" → "…原任务不会自动改用新服务"; 1329/1379/1479 "…请选择服务后建立新尝试" → "…请重新选择服务后再试"; 1214 "修改接口不能改变此服务的用途…" → "不能把此服务改成其他用途，请另外添加服务"
  - service_editor.rs:215 " · v{}" and 1602, 1608 "… · v{} · …" → remove revision numbers
  - 接口类型 / 服务类型 / 协议 mixed (service_editor.rs:759, onboarding.rs:1382, preferences.rs:550 "协议", 1814, service_test.rs:329) → 服务类型
  - model_diagnostics.rs:900 "正在停止…" → "正在暂停…"; 424 "{name} 的准备已停止" → "{name} 的准备已暂停"
  - onboarding.rs:2213 "准备已结束" → "已准备完成" (only when it actually succeeded; failures say so)
  - 检查 vs 测试: service_editor.rs:1215 "检查服务" → "测试服务"; onboarding "检查配置" → "测试…" consistent with settings
  - onboarding.rs:1442 "模型" → "模型 ID"
  - generation_settings.rs:208 button "本机" vs menu item "本机识别" (220) → "本机识别"
  - generation_settings.rs:432 group "保存" → "保存与导出"
  - onboarding.rs:1155 "指定运行方式" → "指定引擎" (and unify 本机引擎/运行方式/引擎 → 引擎)

#### Docs / tooling
- desktop/README.md:56 "Your own checkouts must be clean and are never modified" but sources.py fast-forwards → "are only fast-forwarded, never patched".
- desktop/assets/icon-design/README.md:25 "macOS 14 起" vs LSMinimumSystemVersion 15.0 → 15.
- docs/releases/v2.0.0-rc.7.md: delete (history belongs in git/GitHub; rc.9 stays as release body).
- .github/workflows/aur.yml:74 `${VERSION#v}` redundant.
- .github/workflows/release.yml:125 comment "Desktop tests already ran on the pull request for this revision" inaccurate (tag points at merge commit) → fix comment or run tests.
- deploy-key TODO in two places (brew-tap.yml:42 and docs/PACKAGING.md:22) → keep only the workflow one.
- justfile:19-22 sleep-polls for app exit then rm -rf even after timeout → fail on timeout.
- packaging/bench-mac.sh:24 rm -rf "$OUT" each round on a user-provided dir → run in a mktemp -d subdir.

