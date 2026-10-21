// C ABI for the Apple transcription adapter.
// 供 Rust 侧静态链接；仅 macOS arm64 构建。

import Foundation
import Qwen3ASR
import WhisperASR
import AudioCommon

/// 在同步 C ABI 里执行 async 代码（信号量 + 锁保护结果）。
private final class SyncBox<T>: @unchecked Sendable {
    private var value: Result<T, Error>?
    private let lock = NSLock()
    func set(_ r: Result<T, Error>) { lock.lock(); value = r; lock.unlock() }
    func get() throws -> T { lock.lock(); defer { lock.unlock() }; return try value!.get() }
}

private func runSync<T>(_ body: @escaping @Sendable () async throws -> T) throws -> T {
    let box = SyncBox<T>()
    let sem = DispatchSemaphore(value: 0)
    Task.detached {
        do { box.set(.success(try await body())) } catch { box.set(.failure(error)) }
        sem.signal()
    }
    sem.wait()
    return try box.get()
}

private func loadWav(_ path: String) throws -> [Float] {
    let url = URL(fileURLWithPath: path)
    let (samples, rate) = try AudioFileLoader.loadWAV(url: url)
    if rate == 16000 { return samples }
    return AudioFileLoader.resample(samples, from: rate, to: 16000)
}

// 单线程使用（Rust 侧在同一个阻塞线程内串行调用）。
nonisolated(unsafe) private var lastErrorCStr: UnsafeMutablePointer<CChar>?

private func setErr(_ message: String) {
    message.withCString { c in
        if let old = lastErrorCStr { free(old) }
        lastErrorCStr = strdup(c)
    }
}

/// 最近一次错误的可读描述（指针在下次 setErr 前有效）。
@_cdecl("notepipe_last_error")
public func notepipeLastError() -> UnsafePointer<CChar> {
    if let p = lastErrorCStr { return UnsafePointer(p) }
    return ("unknown error" as NSString).utf8String!
}

@_silgen_name("notepipe_model_progress")
private func reportModelProgress(
    _ progress: Double, _ phase: UnsafePointer<CChar>, _ message: UnsafePointer<CChar>)

private func progressLog(_ p: Double, _ phase: String, _ msg: String) {
    phase.withCString { phasePtr in
        msg.withCString { reportModelProgress(p, phasePtr, $0) }
    }
}

// Rust prepares missing files before loading, and loaders run offline, so every
// progress report here is a local load whatever the library labels it.
private func cachedLoadProgress(_ p: Double, _ msg: String) {
    progressLog(p, "load", "正在加载本机模型… / Loading local model…")
}

// MARK: - ASR：qwen3（CoreML 0.6B/ANE）| qwen3-1.7b（MLX/GPU）| whisper

enum AsrKind {
    case qwen(CoreMLASRModel)
    case qwenMlx(Qwen3ASRModel)
    case whisper(WhisperASRModel)
}

final class AsrBox {
    let model: AsrKind
    init(_ m: AsrKind) { model = m }
}

// Cache-only preparation follows the same repositories and required files as the
// actual loaders. It deliberately avoids constructing a second copy of ASR weights.
@_cdecl("notepipe_asr_prepare_cache")
public func notepipeAsrPrepareCache(
    _ model: UnsafePointer<CChar>,
    _ manifest: UnsafePointer<CChar>
) -> Int32 {
    let name = String(cString: model)
    let manifestText = String(cString: manifest)
    do {
        try runSync {
            let offline = ProcessInfo.processInfo.environment["HF_HUB_OFFLINE"]
                .map { ["1", "true", "yes", "on"].contains($0.lowercased()) } ?? false
            let data = manifestText.data(using: .utf8)!
            guard let items = try JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
                throw NSError(domain: "notepipe", code: 1, userInfo: [
                    NSLocalizedDescriptionKey: "模型清单无效 / Invalid model manifest",
                ])
            }
            let downloads: [(String, [String])] = try items.map { item in
                guard let repo = item["repo"] as? String,
                      let files = item["files"] as? [String] else {
                    throw NSError(domain: "notepipe", code: 1, userInfo: [
                        NSLocalizedDescriptionKey: "模型清单无效 / Invalid model manifest",
                    ])
                }
                return (repo, files)
            }
            for (index, item) in downloads.enumerated() {
                let directory = try HuggingFaceDownloader.getCacheDirectory(for: item.0)
                try await HuggingFaceDownloader.downloadWeights(
                    modelId: item.0,
                    to: directory,
                    additionalFiles: item.1,
                    offlineMode: offline,
                    progressHandler: { fraction in
                        progressLog(
                            (Double(index) + fraction) / Double(downloads.count),
                            "download", item.0)
                    })
            }
        }
        return 0
    } catch {
        setErr("Apple 模型下载失败 / Apple model download failed (\(name)): \(error)")
        return -1
    }
}

@_cdecl("notepipe_asr_create")
public func notepipeAsrCreate(_ model: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    let name = String(cString: model)
    do {
        let kind: AsrKind
        if name == "whisper" {
            let m = try runSync {
                try await WhisperASRModel.fromPretrained(offlineMode: true, progressHandler: cachedLoadProgress)
            }
            kind = .whisper(m)
        } else if name == "qwen3-1.7b" {
            // MLX/GPU 路径（1.7B 无 CoreML/ANE 导出）：WER 约为 0.6B 一半、
            // 速度约 3 倍，代价是 ~2.7GB RSS 且放弃 ANE 低功耗。
            let m = try runSync {
                try await Qwen3ASRModel.fromPretrained(
                    modelId: "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
                    offlineMode: true,
                    progressHandler: cachedLoadProgress)
            }
            kind = .qwenMlx(m)
        } else if name == "qwen3-0.6b" {
            let m = try runSync {
                try await CoreMLASRModel.fromPretrained(offlineMode: true, progressHandler: cachedLoadProgress)
            }
            try m.warmUp()
            kind = .qwen(m)
        } else {
            throw NSError(domain: "notepipe", code: 1, userInfo: [NSLocalizedDescriptionKey: "不支持的 Apple 模型 / Unsupported Apple model: \(name)"])
        }
        return Unmanaged.passRetained(AsrBox(kind)).toOpaque()
    } catch {
        setErr("识别模型加载失败 / Speech model failed to load (\(name)): \(error)")
        return nil
    }
}

@_cdecl("notepipe_asr_transcribe")
public func notepipeAsrTranscribe(
    _ handle: UnsafeMutableRawPointer,
    wavPath: UnsafePointer<CChar>,
    language: UnsafePointer<CChar>?,
    outText: UnsafeMutablePointer<CChar>,
    outLen: Int
) -> Int32 {
    let box = Unmanaged<AsrBox>.fromOpaque(handle).takeUnretainedValue()
    let language = language.map { String(cString: $0) }.flatMap { $0.isEmpty ? nil : $0 }
    do {
        let samples = try loadWav(String(cString: wavPath))
        let text: String
        switch box.model {
        case .qwen(let m):
            text = try m.transcribe(audio: samples, sampleRate: 16000, language: language, maxTokens: 448)
        case .qwenMlx(let m):
            text = m.transcribe(audio: samples, sampleRate: 16000, language: language, maxTokens: 448)
        case .whisper(let m):
            text = try runSync {
                try await m.transcribeAudio(samples, sampleRate: 16000, language: language)
            }
        }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            outText.pointee = 0
            return 1 // 1 = 无文本（非错误）
        }
        let fits = trimmed.utf8CString.count <= outLen // utf8CString 含结尾 NUL
        trimmed.withCString { cStr in
            _ = strncpy(outText, cStr, outLen - 1)
        }
        if !fits {
            outText[outLen - 1] = 0 // strncpy 截断时不保证补 NUL
            return 2 // Rust reports buffer truncation as an error.
        }
        return 0
    } catch {
        setErr("转写失败 / Transcription failed: \(error)")
        return -1
    }
}

@_cdecl("notepipe_asr_destroy")
public func notepipeAsrDestroy(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<AsrBox>.fromOpaque(handle).release()
}
