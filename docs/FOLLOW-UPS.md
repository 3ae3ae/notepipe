# 待办的工程改进

以下改进尚未完成，也不阻塞发布。完成一项就把它从列表中删除，变更理由写在提交说明里。

## 引擎

- `src/dispatch.rs`：每次发送前都要全量读取收据目录，长课程走云端识别时为 O(N²)，需要收据索引。
- `src/models.rs`：2.4 GB 模型下载中断后只留下 `.part`，不会续传（需要 Range 断点续传）。
- 原生 Apple 识别（`native/apple-asr`）手工镜像上游 Hugging Face 仓库名与文件清单；需要 CI 冒烟测试，覆盖「prepare 之后离线加载」。
- 云端识别并发数写死为 4，应改为可配置。
- `failure_message` 按 `" / "` 切分双语字符串，应改为结构化字段。

## 桌面端

- `desktop/src/backend.rs`：日志脱敏依赖硬编码的 JSON 路径去取引擎请求里的密钥；引擎应导出 `secrets()` 访问器。
- `desktop/src/storage.rs`：`sync_all` 只在 Unix 上生效，Windows 上没有对应的落盘保证。
- `service_test.rs` 与 `model_discovery.rs` 各有一套近乎相同的限时 HTTP 请求管道，应合并为一个共享函数。

## 发布

- `HOMEBREW_TAP_DEPLOY_KEY`：Homebrew tap 目前借用 `AUR_SSH_PRIVATE_KEY` 推送到 GitHub。需要为 tap 仓库新建独立的 deploy key 和 secret，再修改 `.github/workflows/brew-tap.yml`。
