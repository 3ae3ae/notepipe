// C ABI for the Apple transcription adapter.
// Linked statically into the Rust binary; macOS arm64 only.

import Foundation
import Qwen3ASR
import WhisperASR
import AudioCommon

/// Runs async code behind the synchronous C ABI (a semaphore, with the result behind a lock).
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

// Used from one thread: Rust calls in sequence from a single blocking thread.
nonisolated(unsafe) private var lastErrorCStr: UnsafeMutablePointer<CChar>?

private func setErr(_ message: String) {
    message.withCString { c in
        if let old = lastErrorCStr { free(old) }
        lastErrorCStr = strdup(c)
    }
}

/// The last error message; the pointer is valid until the next setErr.
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

// MARK: - Models: qwen3-0.6b (Core ML, Neural Engine) | qwen3-1.7b (MLX, GPU) | whisper

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
            // MLX on the GPU (there is no Core ML export of 1.7B): about half the error rate of 0.6B
            // and about 3x faster, at ~2.7 GB of memory and without the Neural Engine's low power.
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
            throw NSError(domain: "notepipe", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unsupported Apple model: \(name)"])
        }
        return Unmanaged.passRetained(AsrBox(kind)).toOpaque()
    } catch {
        setErr("Speech model failed to load (\(name)): \(error)")
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
            return 1 // 1 = no text, not an error
        }
        let fits = trimmed.utf8CString.count <= outLen // utf8CString includes the trailing NUL
        trimmed.withCString { cStr in
            _ = strncpy(outText, cStr, outLen - 1)
        }
        if !fits {
            outText[outLen - 1] = 0 // strncpy does not terminate a truncated copy
            return 2 // Rust reports buffer truncation as an error.
        }
        return 0
    } catch {
        setErr("Transcription failed: \(error)")
        return -1
    }
}

@_cdecl("notepipe_asr_destroy")
public func notepipeAsrDestroy(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<AsrBox>.fromOpaque(handle).release()
}
