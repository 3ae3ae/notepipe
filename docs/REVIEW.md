# 代码审查剩余事项

本文件只保留**仍未完成**的审查项；已完成项已从清单删除，历史记录见 Git 提交。

## 每帧重算与轮询

1. `desktop/src/task_ui.rs` 的 `queue_page` 已缓存排序后的任务快照，但 `recent_notes_section`、`conversion_recovery` 的 `preview_plan`、笔记库筛选/封面、阅读页查找与画廊仍需逐帧缓存或索引。
2. `desktop/src/main.rs` 的任务事件、草稿 debounce、提示过期和退出超时仍由 100/500ms 轮询驱动；应改为 Job 事件、专用定时器和一次性 wake，不再固定轮询。
3. `workspace::transaction` 仍会为每次编辑克隆整个 `State`；后台写入已合并，但内存快照仍可改为增量或写时复制。

## 设置、首次设置与账号

1. `onboarding.rs`、`import_ui.rs` 仍在渲染路径调用 `ensure_model_diagnostic` 并通过 `pending_checks` defer；应在进入步骤、选择模型/服务、刷新环境等事件中触发。
2. `preferences.rs` 的 `#[cfg(test)] resolve_for_execution` / `record_draft_test` 和 `credentials.rs` 的 MemoryVault 轮换测试仍与生产路径并行；测试应走生产函数。
3. `account_ui.rs` 的 `account_summary` 仍重复推导 badge/detail，未统一由 `account_status_text` 渲染；`account_ui.rs`、`service_editor.rs`、`onboarding.rs`、`settings_ui.rs` 的小型文件读写仍在 UI 线程。
4. `generation_settings.rs` 的字幕语言表、“custom” 哨兵、`settings_ui.rs` 的 `llama-server` 名称判断、`onboarding.rs` 的 `"proofread"` id 判断仍按显示/id 字符串分支；应改为类型或共享表。
5. `settings_ui.rs` 的字号选项仍手写 100/125/150/200 与 `FONT_SCALES` 并行；模型选项仍给每个值重复麦克风图标。
6. `service_editor.rs` 的 `close_service_editor` 恒返回 true、`&& has_services` 死分支和若干薄包装仍在。
7. `preferences.rs` 的 `atomic_write_with_retry` 无延迟重试、`StoredTestEvidence` 从详情文本解析 `HTTP n` 的兼容层仍在。

## 原生验证

原生应用尚未逐页覆盖正常/窄/宽窗口、浅色/深色、全部配色、100/125/150/200% 字号，以及加载、空、失败、完成状态；需要在运行中的应用里重新检查设置、服务与账号、首次设置、任务、笔记库和阅读页。
