@preconcurrency import AVFoundation
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "audio-encoder")

/// Microphone PCM (Float32 mono, 48 kHz) to Opus or AAC-LC with Apple's
/// encoders (AVAudioConverter). Input is cut into whole codec frames
/// (`AudioFramer`), one frame per convert call, and each packet's PTS is the
/// capture time of its frame's first sample minus the encoder delay
/// (`AudioTiming.ptsNs`).
///
/// Call `encode` from one serial thread; outputs are delivered on it.
final class AudioEncoder {
    /// Called once the output format is known.
    var onFormat: ((_ sampleRate: Double, _ channels: Int) -> Void)?
    /// Called with one encoded packet (raw AAC frame without ADTS header, or
    /// one Opus packet), its PTS (host ns) and how long the encoder took
    /// from the frame's last sample being in to the packet, ns.
    var onOutput: ((_ packet: Data, _ ptsNs: Int64, _ encodeNs: Int64) -> Void)?

    let codec: AudioCodec
    static let sampleRate: Double = 48000
    private let channels: AVAudioChannelCount = 1
    private var converter: AVAudioConverter?
    private var inputBuffer: AVAudioPCMBuffer?
    private var outputFormat: AVAudioFormat?
    private var framer: AudioFramer
    private var primingFrames: Int
    /// First-sample capture times of frames handed to the converter whose
    /// packets have not come out yet (the AAC encoder may hold one back).
    private var pendingFrames: [Int64] = []
    private var maxPacketSize = 1536

    init(codec: AudioCodec) {
        self.codec = codec
        framer = AudioFramer(frameLength: codec.frameLength, sampleRate: Self.sampleRate)
        primingFrames = codec.defaultPrimingFrames
    }

    /// - Parameters:
    ///   - samples: Mono samples at `sampleRate`.
    ///   - captureNs: Capture time of `samples[0]`, host ns.
    func encode(_ samples: UnsafeBufferPointer<Float>, captureNs: Int64, sampleRate: Double) {
        guard sampleRate == Self.sampleRate else {
            log.error("Microphone at \(sampleRate) Hz, the encoder needs \(Self.sampleRate)")
            return
        }
        if converter == nil, !setUpConverter() {
            return
        }
        framer.push(samples, captureNs: captureNs) { frame, firstNs in
            encodeFrame(frame, firstSampleNs: firstNs)
        }
    }

    private func encodeFrame(_ frame: UnsafeBufferPointer<Float>, firstSampleNs: Int64) {
        guard let converter, let inputBuffer, let outputFormat,
              let channel = inputBuffer.floatChannelData?[0]
        else {
            return
        }
        let startNs = HostClock.nowNs()
        for i in 0 ..< frame.count {
            channel[i] = frame[i]
        }
        inputBuffer.frameLength = AVAudioFrameCount(frame.count)
        pendingFrames.append(firstSampleNs)
        var provided = false
        while true {
            let output = AVAudioCompressedBuffer(format: outputFormat, packetCapacity: 1,
                                                 maximumPacketSize: maxPacketSize)
            var error: NSError?
            let status = converter.convert(to: output, error: &error) { _, inputStatus in
                if provided {
                    inputStatus.pointee = .noDataNow
                    return nil
                }
                provided = true
                inputStatus.pointee = .haveData
                return inputBuffer
            }
            if status == .error {
                log.error("Encode failed: \(String(describing: error))")
                pendingFrames.removeAll()
                return
            }
            if output.packetCount > 0, output.byteLength > 0, !pendingFrames.isEmpty {
                let first = pendingFrames.removeFirst()
                let pts = AudioTiming.ptsNs(firstSampleNs: first, primingFrames: primingFrames,
                                            sampleRate: Self.sampleRate)
                onOutput?(Data(bytes: output.data, count: Int(output.byteLength)), pts, HostClock.nowNs() - startNs)
            }
            if status != .haveData {
                break
            }
        }
        // A converter that swallowed frames without output must not shift
        // the timestamps of later packets by more than its own delay.
        if pendingFrames.count > 4 {
            pendingFrames.removeFirst(pendingFrames.count - 4)
        }
    }

    private func setUpConverter() -> Bool {
        guard let inputFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Self.sampleRate,
                                              channels: channels, interleaved: false)
        else {
            return false
        }
        var description = switch codec {
        case .opus:
            AudioStreamBasicDescription(mSampleRate: Self.sampleRate, mFormatID: kAudioFormatOpus,
                                        mFormatFlags: 0, mBytesPerPacket: 0,
                                        mFramesPerPacket: UInt32(codec.frameLength), mBytesPerFrame: 0,
                                        mChannelsPerFrame: channels, mBitsPerChannel: 0, mReserved: 0)
        case .aac:
            AudioStreamBasicDescription(mSampleRate: Self.sampleRate, mFormatID: kAudioFormatMPEG4AAC,
                                        mFormatFlags: UInt32(MPEG4ObjectID.AAC_LC.rawValue), mBytesPerPacket: 0,
                                        mFramesPerPacket: UInt32(codec.frameLength), mBytesPerFrame: 0,
                                        mChannelsPerFrame: channels, mBitsPerChannel: 0, mReserved: 0)
        }
        guard let outputFormat = AVAudioFormat(streamDescription: &description),
              let converter = AVAudioConverter(from: inputFormat, to: outputFormat),
              let inputBuffer = AVAudioPCMBuffer(pcmFormat: inputFormat,
                                                 frameCapacity: AVAudioFrameCount(codec.frameLength))
        else {
            log.error("No \(self.codec.label) encoder for \(Self.sampleRate) Hz mono")
            return false
        }
        converter.bitRate = Self.closestBitrate(codec.bitrate, in: converter.applicableEncodeBitRates)
        // Opus: the encoder's pre-skip. AAC: Apple's converter reports 0,
        // its decoders' 2112 samples are the known value.
        let reported = Int(converter.primeInfo.leadingFrames)
        if codec == .opus, reported > 0 {
            primingFrames = reported
        }
        maxPacketSize = max(converter.maximumOutputPacketSize, 1536)
        log.info("\(self.codec.label) \(Self.sampleRate) Hz, \(self.channels) ch, \(converter.bitRate) bit/s, \(self.codec.frameLength)-sample frames, priming \(self.primingFrames) (converter reports \(reported))")
        self.converter = converter
        self.inputBuffer = inputBuffer
        self.outputFormat = outputFormat
        framer.reset()
        pendingFrames.removeAll()
        onFormat?(Self.sampleRate, Int(channels))
        return true
    }

    private static func closestBitrate(_ wanted: Int, in applicable: [NSNumber]?) -> Int {
        let rates = (applicable ?? []).map(\.intValue)
        guard !rates.isEmpty else {
            return wanted
        }
        return rates.filter { $0 <= wanted }.max() ?? rates.min() ?? wanted
    }
}
