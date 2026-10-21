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
private func reportModelProgress(_ progress: Double, _ message: UnsafePointer<CChar>)

private func modelProgress(_ progress: Double, _ message: String) {
    message.withCString { reportModelProgress(progress, $0) }
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

@_cdecl("notepipe_asr_create")
public func notepipeAsrCreate(_ model: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    let name = String(cString: model)
    let offline = ProcessInfo.processInfo.environment["HF_HUB_OFFLINE"]
        .map { ["1", "true", "yes", "on"].contains($0.lowercased()) } ?? false
    do {
        let kind: AsrKind
        if name == "whisper" {
            let m = try runSync {
                try await WhisperASRModel.fromPretrained(offlineMode: offline, progressHandler: modelProgress)
            }
            kind = .whisper(m)
        } else if name == "qwen3-1.7b" {
            // MLX/GPU 路径（1.7B 无 CoreML/ANE 导出）：WER 约为 0.6B 一半、
            // 速度约 3 倍，代价是 ~2.7GB RSS 且放弃 ANE 低功耗。
            let m = try runSync {
                try await Qwen3ASRModel.fromPretrained(
                    modelId: "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
                    offlineMode: offline,
                    progressHandler: modelProgress)
            }
            kind = .qwenMlx(m)
        } else if name == "qwen3-0.6b" {
            let m = try runSync {
                try await CoreMLASRModel.fromPretrained(offlineMode: offline, progressHandler: modelProgress)
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
