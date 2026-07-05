import CoreAudio
import Darwin
import Foundation

struct TapError: Error, CustomStringConvertible {
    let operation: String
    let status: OSStatus

    var description: String {
        "\(operation) failed: \(describeOSStatus(status))"
    }
}

struct Options {
    var seconds: Double = 10
    var outputPath: String = "process_tap_ref.wav"
    var mono = false
    var streamStdout = false
    var probePermission = false
    var preflightPermission = false
    var excludePids: [pid_t] = []
}

func usage() -> Never {
    fputs(
        """
        Usage:
          echoless-process-tap-poc [--seconds N] [--out PATH] [--mono]
          echoless-process-tap-poc --stream-stdout [--mono] [--exclude-pid PID ...]
          echoless-process-tap-poc --probe-permission [--mono]
          echoless-process-tap-poc --preflight-permission

        Captures macOS system output audio through Core Audio Process Tap.
        Play audio while this runs, then inspect the generated WAV.
        --stream-stdout writes raw little-endian Float32 PCM to stdout for the
        Echoless realtime pipeline.
        --probe-permission starts and stops a tap without writing audio. Use it
        only after explicit user action to trigger System Audio Recording TCC.
        --preflight-permission prints granted|denied|undetermined|unknown to
        stdout without ever triggering the TCC prompt (private TCC SPI).
        --exclude-pid excludes an audio-producing process, usually the parent
        echoless CLI, so processed output does not contaminate the reference.

        """,
        stderr
    )
    exit(2)
}

func parseOptions() -> Options {
    var options = Options()
    var index = 1
    let args = CommandLine.arguments
    while index < args.count {
        switch args[index] {
        case "--seconds":
            index += 1
            guard index < args.count, let seconds = Double(args[index]), seconds > 0 else {
                usage()
            }
            options.seconds = seconds
        case "--out":
            index += 1
            guard index < args.count else { usage() }
            options.outputPath = args[index]
        case "--mono":
            options.mono = true
        case "--stream-stdout":
            options.streamStdout = true
        case "--probe-permission":
            options.probePermission = true
        case "--preflight-permission":
            options.preflightPermission = true
        case "--exclude-pid":
            index += 1
            guard index < args.count, let pid = Int32(args[index]), pid > 0 else {
                usage()
            }
            options.excludePids.append(pid_t(pid))
        case "-h", "--help":
            usage()
        default:
            usage()
        }
        index += 1
    }
    return options
}

func check(_ status: OSStatus, _ operation: String) throws {
    guard status == noErr else { throw TapError(operation: operation, status: status) }
}

func describeOSStatus(_ status: OSStatus) -> String {
    let unsigned = UInt32(bitPattern: status)
    let chars = [
        UInt8((unsigned >> 24) & 0xff),
        UInt8((unsigned >> 16) & 0xff),
        UInt8((unsigned >> 8) & 0xff),
        UInt8(unsigned & 0xff),
    ]
    if chars.allSatisfy({ $0 >= 32 && $0 <= 126 }) {
        return "\(status) ('\(String(bytes: chars, encoding: .ascii) ?? "????")')"
    }
    return "\(status)"
}

func propertyAddress(
    _ selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
}

func getTapFormat(_ tapID: AudioObjectID) throws -> AudioStreamBasicDescription {
    var address = propertyAddress(kAudioTapPropertyFormat)
    var format = AudioStreamBasicDescription()
    var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
    let status = AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &format)
    try check(status, "AudioObjectGetPropertyData(kAudioTapPropertyFormat)")
    return format
}

func processObjectID(forPID pid: pid_t) throws -> AudioObjectID {
    var address = propertyAddress(kAudioHardwarePropertyTranslatePIDToProcessObject)
    var pidValue = pid
    var processObjectID = AudioObjectID(kAudioObjectUnknown)
    var size = UInt32(MemoryLayout<AudioObjectID>.size)
    let status = withUnsafePointer(to: &pidValue) { pidPointer in
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            UInt32(MemoryLayout<pid_t>.size),
            pidPointer,
            &size,
            &processObjectID
        )
    }
    try check(status, "AudioObjectGetPropertyData(kAudioHardwarePropertyTranslatePIDToProcessObject)")
    return processObjectID
}

func processObjectIDs(forPids pids: [pid_t]) throws -> [AudioObjectID] {
    var processObjectIDs: [AudioObjectID] = []
    for pid in pids {
        let processObjectID = try processObjectID(forPID: pid)
        if processObjectID == kAudioObjectUnknown {
            fputs("warning: PID \(pid) is not a Core Audio process object; cannot exclude it\n", stderr)
            continue
        }
        fputs("excluding PID \(pid) from Process Tap as process object \(processObjectID)\n", stderr)
        processObjectIDs.append(processObjectID)
    }
    return processObjectIDs
}

func formatSummary(_ format: AudioStreamBasicDescription) -> String {
    let isFloat = (format.mFormatFlags & kAudioFormatFlagIsFloat) != 0
    let isNonInterleaved = (format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
    return String(
        format:
            "%.0f Hz, %u ch, %u-bit %@%@",
        format.mSampleRate,
        format.mChannelsPerFrame,
        format.mBitsPerChannel,
        isFloat ? "float" : "pcm",
        isNonInterleaved ? ", non-interleaved" : ", interleaved"
    )
}

final class ProcessTapRecorder {
    private let options: Options
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var format = AudioStreamBasicDescription()
    private let lock = NSLock()
    private var interleavedSamples: [Float] = []
    private var capturedFrames: Int64 = 0
    private var callbackCount: Int64 = 0
    private var firstHostTime: UInt64?
    private var lastHostTime: UInt64?
    private var streamData = Data()

    init(options: Options) {
        self.options = options
    }

    var sampleRate: Double {
        format.mSampleRate
    }

    var channels: Int {
        Int(format.mChannelsPerFrame)
    }

    var formatDescription: String {
        formatSummary(format)
    }

    func start() throws {
        if #unavailable(macOS 14.2) {
            throw TapError(operation: "Process Tap availability", status: -1)
        }

        let excludedProcessIDs = try processObjectIDs(forPids: options.excludePids)
        let description: CATapDescription
        if options.mono {
            description = CATapDescription(monoGlobalTapButExcludeProcesses: excludedProcessIDs)
        } else {
            description = CATapDescription(stereoGlobalTapButExcludeProcesses: excludedProcessIDs)
        }
        description.name = "Echoless Process Tap PoC"
        description.isPrivate = true
        description.muteBehavior = .unmuted

        var createdTapID = AudioObjectID(kAudioObjectUnknown)
        try check(
            AudioHardwareCreateProcessTap(description, &createdTapID),
            "AudioHardwareCreateProcessTap"
        )
        tapID = createdTapID
        guard tapID != kAudioObjectUnknown else {
            throw TapError(operation: "AudioHardwareCreateProcessTap returned unknown object", status: -3)
        }

        let tapUID = description.uuid.uuidString as CFString
        format = try getTapFormat(tapID)
        guard format.mFormatID == kAudioFormatLinearPCM,
              (format.mFormatFlags & kAudioFormatFlagIsFloat) != 0,
              format.mBitsPerChannel == 32
        else {
            throw TapError(operation: "Unsupported tap format: \(formatSummary(format))", status: -2)
        }

        let aggregateDescription: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Echoless Process Tap PoC",
            kAudioAggregateDeviceUIDKey: "com.echoless.process-tap-poc.\(UUID().uuidString)",
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceTapAutoStartKey: false,
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapUIDKey: tapUID,
                    kAudioSubTapDriftCompensationKey: true,
                ],
            ],
        ]

        var createdAggregateID = AudioObjectID(kAudioObjectUnknown)
        try check(
            AudioHardwareCreateAggregateDevice(
                aggregateDescription as CFDictionary,
                &createdAggregateID
            ),
            "AudioHardwareCreateAggregateDevice"
        )
        aggregateID = createdAggregateID

        try check(
            AudioDeviceCreateIOProcIDWithBlock(&ioProcID, aggregateID, nil) {
                [weak self] _, inputData, inputTime, _, _ in
                self?.capture(inputData: inputData, inputTime: inputTime)
            },
            "AudioDeviceCreateIOProcIDWithBlock"
        )

        try check(AudioDeviceStart(aggregateID, ioProcID), "AudioDeviceStart")
    }

    func stop() {
        if let ioProcID {
            _ = AudioDeviceStop(aggregateID, ioProcID)
            _ = AudioDeviceDestroyIOProcID(aggregateID, ioProcID)
        }
        ioProcID = nil

        if aggregateID != kAudioObjectUnknown {
            _ = AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        aggregateID = AudioObjectID(kAudioObjectUnknown)

        if tapID != kAudioObjectUnknown {
            _ = AudioHardwareDestroyProcessTap(tapID)
        }
        tapID = AudioObjectID(kAudioObjectUnknown)
    }

    func snapshot() -> (frames: Int64, callbacks: Int64, peak: Float, rms: Float) {
        lock.lock()
        defer { lock.unlock() }

        var sumSquares: Float = 0
        var peak: Float = 0
        for sample in interleavedSamples {
            let absValue = abs(sample)
            peak = max(peak, absValue)
            sumSquares += sample * sample
        }
        let rms = interleavedSamples.isEmpty
            ? 0
            : sqrt(sumSquares / Float(interleavedSamples.count))
        return (capturedFrames, callbackCount, peak, rms)
    }

    func drainStreamData() -> Data {
        lock.lock()
        defer { lock.unlock() }

        let data = streamData
        streamData.removeAll(keepingCapacity: true)
        return data
    }

    func writeWav(to path: String) throws {
        lock.lock()
        let samples = interleavedSamples
        let frames = capturedFrames
        let callbacks = callbackCount
        let firstHost = firstHostTime
        let lastHost = lastHostTime
        lock.unlock()

        try writeFloat32Wav(
            path: path,
            samples: samples,
            sampleRate: UInt32(format.mSampleRate.rounded()),
            channels: UInt16(format.mChannelsPerFrame)
        )

        let hostRange = if let firstHost, let lastHost {
            "\(firstHost)..\(lastHost)"
        } else {
            "n/a"
        }
        fputs(
            "wrote \(path) frames=\(frames) callbacks=\(callbacks) hostTime=\(hostRange)\n",
            stderr
        )
    }

    private func capture(
        inputData: UnsafePointer<AudioBufferList>?,
        inputTime: UnsafePointer<AudioTimeStamp>?
    ) {
        guard let inputData else { return }

        let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inputData))
        let channelCount = max(1, Int(format.mChannelsPerFrame))
        var chunk: [Float] = []
        var frames = 0

        if buffers.count == 1,
           let buffer = buffers.first,
           Int(buffer.mNumberChannels) == channelCount,
           let data = buffer.mData
        {
            let sampleCount = Int(buffer.mDataByteSize) / MemoryLayout<Float>.size
            let ptr = data.assumingMemoryBound(to: Float.self)
            chunk.append(contentsOf: UnsafeBufferPointer(start: ptr, count: sampleCount))
            frames = sampleCount / channelCount
        } else {
            var channelPointers: [UnsafePointer<Float>] = []
            var channelFrames = Int.max
            for buffer in buffers {
                guard let data = buffer.mData else { continue }
                let sampleCount = Int(buffer.mDataByteSize) / MemoryLayout<Float>.size
                channelPointers.append(UnsafePointer(data.assumingMemoryBound(to: Float.self)))
                channelFrames = min(channelFrames, sampleCount)
            }

            guard !channelPointers.isEmpty, channelFrames != Int.max else { return }
            frames = channelFrames
            chunk.reserveCapacity(frames * channelCount)
            for frame in 0..<frames {
                for channel in 0..<channelCount {
                    let ptr = channelPointers[min(channel, channelPointers.count - 1)]
                    chunk.append(ptr[frame])
                }
            }
        }

        guard frames > 0 else { return }

        lock.lock()
        if options.streamStdout {
            appendStreamData(samples: chunk)
        } else {
            interleavedSamples.append(contentsOf: chunk)
        }
        capturedFrames += Int64(frames)
        callbackCount += 1
        if let hostTime = inputTime?.pointee.mHostTime, hostTime != 0 {
            if firstHostTime == nil {
                firstHostTime = hostTime
            }
            lastHostTime = hostTime
        }
        lock.unlock()
    }

    private func appendStreamData(samples: [Float]) {
        streamData.reserveCapacity(streamData.count + samples.count * MemoryLayout<Float>.size)
        for sample in samples {
            var bits = sample.bitPattern.littleEndian
            streamData.append(Data(bytes: &bits, count: MemoryLayout<UInt32>.size))
        }
    }
}

func writeFloat32Wav(
    path: String,
    samples: [Float],
    sampleRate: UInt32,
    channels: UInt16
) throws {
    let dataBytes = UInt32(samples.count * MemoryLayout<Float>.size)
    let byteRate = sampleRate * UInt32(channels) * 4
    let blockAlign = channels * 4
    var data = Data()

    func appendString(_ value: String) {
        data.append(value.data(using: .ascii)!)
    }
    func appendUInt16(_ value: UInt16) {
        var little = value.littleEndian
        data.append(Data(bytes: &little, count: MemoryLayout<UInt16>.size))
    }
    func appendUInt32(_ value: UInt32) {
        var little = value.littleEndian
        data.append(Data(bytes: &little, count: MemoryLayout<UInt32>.size))
    }
    func appendFloat32(_ value: Float) {
        var bits = value.bitPattern.littleEndian
        data.append(Data(bytes: &bits, count: MemoryLayout<UInt32>.size))
    }

    appendString("RIFF")
    appendUInt32(36 + dataBytes)
    appendString("WAVE")
    appendString("fmt ")
    appendUInt32(16)
    appendUInt16(3)
    appendUInt16(channels)
    appendUInt32(sampleRate)
    appendUInt32(byteRate)
    appendUInt16(blockAlign)
    appendUInt16(32)
    appendString("data")
    appendUInt32(dataBytes)
    for sample in samples {
        appendFloat32(sample)
    }

    try data.write(to: URL(fileURLWithPath: path), options: .atomic)
}

// 无弹窗查询 System Audio Recording(kTCCServiceAudioCapture)授权状态。
// TCC 没有公开的查询 API;这里走私有 TCC.framework 的 TCCAccessPreflight
//(AudioCap 同款做法)。任何一步失败都回退 "unknown",绝不触发授权弹窗。
func preflightAudioCapturePermission() -> String {
    guard let handle = dlopen(
        "/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC",
        RTLD_NOW
    ) else { return "unknown" }
    defer { dlclose(handle) }
    guard let sym = dlsym(handle, "TCCAccessPreflight") else { return "unknown" }
    typealias PreflightFunc = @convention(c) (CFString, CFDictionary?) -> Int32
    let preflight = unsafeBitCast(sym, to: PreflightFunc.self)
    // 私有 API 的返回枚举只有 0=granted 可确证;1/2 的 denied/unknown 语义在
    // 不同 headers dump 里互相矛盾(2026-07-05 实测:无授权记录时返回 1,若按
    // "denied" 上报,UI 会引导去系统设置——而那里没有条目可开,请求弹窗的路径
    // 反而永远走不到,用户卡死)。故非 0 一律报 undetermined:UI 走「请求」路径,
    // 真 denied 时系统静默不弹窗,再由前端兜底打开设置,无一卡死。
    switch preflight("kTCCServiceAudioCapture" as CFString, nil) {
    case 0: return "granted"
    case 1, 2: return "undetermined"
    default: return "unknown"
    }
}

// TCC 责任人自立(disclaim,AudioCap 同款架构)。不做这步时,授权归属启动链
// 上层的 responsible process —— dev 下就是终端 App(Cursor/Warp/Terminal):
// 终端一自动更新记录即失效(面板里开关还开着但签名失配),Warp 这类缺
// NSAudioCaptureUsageDescription 的终端更是连弹窗都出不来(2026-07-05 日志实证)。
// self-disclaim 后授权记在本 helper 自己名下(内嵌 Info.plist 提供 bundle id
// 与用途说明),与终端、与 Echoless app 的重编彻底解耦。
func selfDisclaimIfNeeded() {
    let marker = "ECHOLESS_TAP_DISCLAIMED"
    if ProcessInfo.processInfo.environment[marker] != nil { return }
    guard let sym = dlsym(dlopen(nil, RTLD_NOW), "responsibility_spawnattrs_setdisclaim") else {
        return
    }
    typealias SetDisclaimFunc = @convention(c) (
        UnsafeMutablePointer<posix_spawnattr_t?>, Int32
    ) -> Int32
    let setDisclaim = unsafeBitCast(sym, to: SetDisclaimFunc.self)

    var attr: posix_spawnattr_t? = nil
    guard posix_spawnattr_init(&attr) == 0 else { return }
    defer { posix_spawnattr_destroy(&attr) }
    guard setDisclaim(&attr, 1) == 0,
        posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_SETEXEC)) == 0
    else { return }

    // SETEXEC:用带 disclaim 的自身镜像原地替换当前进程(pid/stdio 不变),
    // 环境标记防循环;posix_spawn 成功即不返回,失败则以未 disclaim 状态继续。
    let exePath = Bundle.main.executablePath ?? CommandLine.arguments[0]
    var argv: [UnsafeMutablePointer<CChar>?] = CommandLine.arguments.map { strdup($0) }
    argv.append(nil)
    var envs: [UnsafeMutablePointer<CChar>?] = ProcessInfo.processInfo.environment.map {
        strdup("\($0.key)=\($0.value)")
    }
    envs.append(strdup("\(marker)=1"))
    envs.append(nil)
    _ = posix_spawn(nil, exePath, nil, &attr, argv, envs)
}
selfDisclaimIfNeeded()

let options = parseOptions()

if options.preflightPermission {
    print(preflightAudioCapturePermission())
    exit(0)
}

// 显式触发「系统音频录制」授权弹窗(私有 TCCAccessRequest,AudioCap 同款)。
// 关键事实(2026-07-05 日志实证):CATap/aggregate 的创建与启动只会让 coreaudiod
// 向 tccd 发 preflight 查询,**永远不会自己弹授权框** —— 之前靠「建 tap 顺便触发
// 弹窗」的假设是错的。返回 nil = SPI 不可用(继续走 tap 尝试兜底)。
// 注意:弹窗归属于 responsible process(启动 dev 的终端);该 app 的 Info.plist
// 需含 NSAudioCaptureUsageDescription(Cursor 有,Warp 截至今日没有)。
func requestAudioCapturePermission() -> Bool? {
    guard let handle = dlopen(
        "/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC",
        RTLD_NOW
    ) else { return nil }
    defer { dlclose(handle) }
    guard let sym = dlsym(handle, "TCCAccessRequest") else { return nil }
    typealias RequestFunc = @convention(c) (
        CFString, CFDictionary?, @escaping @convention(block) (Bool) -> Void
    ) -> Void
    let request = unsafeBitCast(sym, to: RequestFunc.self)
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false
    request("kTCCServiceAudioCapture" as CFString, nil) { ok in
        granted = ok
        semaphore.signal()
    }
    // 等用户在弹窗上做决定;超时按未授予处理。
    if semaphore.wait(timeout: .now() + 120) == .timedOut {
        fputs("permission request timed out waiting for user\n", stderr)
        return false
    }
    return granted
}

if options.probePermission {
    if let granted = requestAudioCapturePermission(), !granted {
        fputs("system audio recording permission was not granted\n", stderr)
        exit(1)
    }
    // granted 或 SPI 不可用:继续用真实 tap 启停验证一次。
}

let recorder = ProcessTapRecorder(options: options)

do {
    try recorder.start()
    fputs("started Process Tap: \(recorder.formatDescription)\n", stderr)
} catch {
    fputs("failed to start Process Tap: \(error)\n", stderr)
    recorder.stop()
    exit(1)
}

if options.probePermission {
    Thread.sleep(forTimeInterval: 0.1)
    recorder.stop()
    fputs("permission probe succeeded\n", stderr)
    exit(0)
} else if options.streamStdout {
    // A5:PCM 前先发 16 字节二进制头(magic ELTP + version + 实际采样率 + 声道数),
    // Rust 侧据此决定是否插重采样 —— tap 采样率跟随系统输出设备,不恒为 48k。
    var header = Data("ELTP".utf8)
    for value in [UInt32(1), UInt32(recorder.sampleRate.rounded()), UInt32(recorder.channels)] {
        var le = value.littleEndian
        withUnsafeBytes(of: &le) { header.append(contentsOf: $0) }
    }
    FileHandle.standardOutput.write(header)
    fputs("streaming raw Float32 PCM to stdout (\(recorder.formatDescription))\n", stderr)
    while true {
        Thread.sleep(forTimeInterval: 0.005)
        let data = recorder.drainStreamData()
        if !data.isEmpty {
            FileHandle.standardOutput.write(data)
        }
    }
} else {
    // The tap is active only after AudioDeviceStart. The first start is the point
    // where macOS should request System Audio Recording permission for this binary.
    fputs(
        "recording \(String(format: "%.1f", options.seconds)) seconds to \(options.outputPath); play system audio now...\n",
        stderr
    )

    let start = Date()
    while Date().timeIntervalSince(start) < options.seconds {
        Thread.sleep(forTimeInterval: 0.5)
        let stats = recorder.snapshot()
        fputs(
            String(
                format: "frames=%lld callbacks=%lld peak=%.5f rms=%.5f\n",
                stats.frames,
                stats.callbacks,
                stats.peak,
                stats.rms
            ),
            stderr
        )
    }

    recorder.stop()

    do {
        try recorder.writeWav(to: options.outputPath)
    } catch {
        fputs("failed to write WAV: \(error)\n", stderr)
        exit(1)
    }
}
