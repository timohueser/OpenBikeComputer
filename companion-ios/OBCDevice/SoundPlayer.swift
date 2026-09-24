import AVFoundation
import os

/// The phone speaker as the device's sounder: one player node, and a new cue interrupts the one
/// that plays.
///
/// The session is `.playback` with `.mixWithOthers`, so a cue plays with the silent switch on and
/// over the rider's music instead of stopping it.
@MainActor
final class SoundPlayer {
    private static let log = Logger(subsystem: "com.openbikecomputer.device", category: "sound")
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    /// Mono float at the output rate. Nil until the first `start` finds an output.
    private var format: AVAudioFormat?

    init() {
        engine.attach(player)
    }

    /// The rate the host renders a cue at. 0 without an output, and the host then gives no samples.
    var sampleRate: UInt32 { UInt32(format?.sampleRate ?? 0) }

    /// Start or resume the output. A failure is logged and leaves the device silent, never stopped.
    func start() {
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playback, options: [.mixWithOthers])
            try session.setActive(true)
            if format == nil {
                // The output rate is known only once the session is active.
                let rate = engine.outputNode.outputFormat(forBus: 0).sampleRate
                guard rate > 0, let mono = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1) else {
                    Self.log.error("No audio output format at \(rate) Hz; cues stay silent")
                    return
                }
                engine.connect(player, to: engine.mainMixerNode, format: mono)
                format = mono
            }
            try engine.start()
            // Only on a running engine: `play` on a stopped one raises an exception.
            player.play()
        } catch {
            Self.log.error("The audio output did not start: \(error.localizedDescription, privacy: .public)")
        }
    }

    func pause() {
        engine.pause()
    }

    /// Play `count` samples at `sampleRate`. The new cue stops the cue that plays.
    func play(_ samples: UnsafePointer<Float>, count: Int) {
        // An interruption or a route change stops the engine by itself.
        if !engine.isRunning { start() }
        guard engine.isRunning, let format, count > 0,
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count)),
            let channel = buffer.floatChannelData?[0]
        else { return }
        channel.update(from: samples, count: count)
        buffer.frameLength = AVAudioFrameCount(count)
        player.scheduleBuffer(buffer, at: nil, options: .interrupts, completionHandler: nil)
    }
}
