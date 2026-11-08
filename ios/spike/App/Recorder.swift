import AVFoundation

/// Keeps the microphone open during a session, which is what keeps the app
/// alive in the background. Audio is held only between `beginCapture` and
/// `endCapture`, apart from a short rolling buffer so first words aren't cut.
final class Recorder {
    var onStatus: (String) -> Void = { _ in }

    private static let idleLimit: TimeInterval = 300
    private static let bytesPerSecond = 32_000
    private static let preRollLimit = bytesPerSecond * 2 / 5
    private static let captureLimit = bytesPerSecond * 120

    private let engine = AVAudioEngine()
    private let targetFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 16_000,
        channels: 1,
        interleaved: true
    )!
    private var converter: AVAudioConverter?
    private var wantsRunning = false
    private var idleTimer: Timer?

    // Touched only on `queue`.
    private let queue = DispatchQueue(label: "vox.recorder")
    private var preRoll = Data()
    private var captured = Data()
    private var isCapturing = false

    var isRunning: Bool { engine.isRunning }

    init() {
        let center = NotificationCenter.default
        center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: nil,
            queue: .main
        ) { [weak self] note in
            self?.handleInterruption(note)
        }
        center.addObserver(
            forName: .AVAudioEngineConfigurationChange,
            object: engine,
            queue: .main
        ) { [weak self] _ in
            self?.restart()
        }
    }

    func start() {
        AVAudioApplication.requestRecordPermission { granted in
            DispatchQueue.main.async {
                guard granted else {
                    self.onStatus("Microphone permission denied")
                    return
                }
                self.wantsRunning = true
                self.begin()
            }
        }
    }

    func stop(reason: String) {
        wantsRunning = false
        idleTimer?.invalidate()
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        cancelCapture()
        onStatus(reason)
    }

    func beginCapture() {
        resetIdleTimer()
        queue.async {
            self.captured = self.preRoll
            self.isCapturing = true
        }
    }

    func cancelCapture() {
        queue.async {
            self.isCapturing = false
            self.captured = Data()
        }
    }

    /// Hands back the captured speech as a WAV file.
    func endCapture(completion: @escaping (Data) -> Void) {
        resetIdleTimer()
        queue.async {
            self.isCapturing = false
            let pcm = self.captured
            self.captured = Data()
            completion(Self.wav(pcm: pcm))
        }
    }

    private func begin() {
        guard !engine.isRunning else {
            resetIdleTimer()
            return
        }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, options: [.mixWithOthers])
            try session.setActive(true)

            let input = engine.inputNode
            let format = input.outputFormat(forBus: 0)
            guard format.sampleRate > 0 else {
                onStatus("Microphone unavailable")
                return
            }
            converter = AVAudioConverter(from: format, to: targetFormat)
            input.removeTap(onBus: 0)
            input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
                self?.process(buffer)
            }
            engine.prepare()
            try engine.start()
            onStatus("Running")
            resetIdleTimer()
        } catch {
            onStatus("Failed: \(error.localizedDescription)")
        }
    }

    private func restart() {
        guard wantsRunning else { return }
        engine.stop()
        begin()
    }

    private func handleInterruption(_ note: Notification) {
        guard
            let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: raw)
        else {
            return
        }
        switch type {
        case .began:
            onStatus("Interrupted")
        case .ended:
            restart()
        @unknown default:
            break
        }
    }

    private func resetIdleTimer() {
        idleTimer?.invalidate()
        idleTimer = Timer.scheduledTimer(withTimeInterval: Self.idleLimit, repeats: false) {
            [weak self] _ in
            self?.stop(reason: "Off after 5 idle minutes")
        }
    }

    private func process(_ buffer: AVAudioPCMBuffer) {
        guard let chunk = convert(buffer) else { return }
        queue.async {
            if self.isCapturing, self.captured.count < Self.captureLimit {
                self.captured.append(chunk)
            }
            self.preRoll.append(chunk)
            if self.preRoll.count > Self.preRollLimit {
                self.preRoll = Data(self.preRoll.suffix(Self.preRollLimit))
            }
        }
    }

    private func convert(_ buffer: AVAudioPCMBuffer) -> Data? {
        guard let converter else { return nil }
        let ratio = targetFormat.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 16
        guard let output = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: capacity) else {
            return nil
        }

        var supplied = false
        var failure: NSError?
        converter.convert(to: output, error: &failure) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return buffer
        }
        guard failure == nil, let samples = output.int16ChannelData else { return nil }
        return Data(bytes: samples[0], count: Int(output.frameLength) * 2)
    }

    private static func wav(pcm: Data) -> Data {
        func le32(_ value: Int) -> Data {
            withUnsafeBytes(of: UInt32(value).littleEndian) { Data($0) }
        }
        func le16(_ value: Int) -> Data {
            withUnsafeBytes(of: UInt16(value).littleEndian) { Data($0) }
        }

        var file = Data("RIFF".utf8)
        file += le32(36 + pcm.count)
        file += Data("WAVEfmt ".utf8)
        file += le32(16)
        file += le16(1)
        file += le16(1)
        file += le32(16_000)
        file += le32(bytesPerSecond)
        file += le16(2)
        file += le16(16)
        file += Data("data".utf8)
        file += le32(pcm.count)
        file += pcm
        return file
    }
}
