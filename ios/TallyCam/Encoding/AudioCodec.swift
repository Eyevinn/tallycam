import Foundation

/// The audio codec of the stream, chosen with the LCP control `audio_codec`
/// (applies on the next Go Live).
enum AudioCodec: String, CaseIterable {
    /// Opus, 10 ms frames, in MPEG-TS as private data (registration 'Opus').
    case opus
    /// AAC-LC, 1024-sample frames, ADTS in MPEG-TS.
    case aac

    static let defaultCodec = AudioCodec.opus

    var label: String {
        switch self {
        case .opus: "Opus"
        case .aac: "AAC"
        }
    }

    /// Samples per encoded frame at 48 kHz.
    var frameLength: Int {
        switch self {
        case .opus: 480
        case .aac: 1024
        }
    }

    var bitrate: Int {
        switch self {
        case .opus: 80000
        case .aac: 128_000
        }
    }

    /// Encoder delay in samples when the encoder does not report one. The
    /// decoder's output starts this many samples before the first input
    /// sample of the first frame; decoders do not skip it in MPEG-TS, so
    /// the PTS are moved back by it (see `AudioTiming.ptsNs`).
    var defaultPrimingFrames: Int {
        switch self {
        // Opus pre-skip of the standard (non restricted-lowdelay) modes:
        // 6.5 ms. Apple's encoder reports 312 as well.
        case .opus: 312
        // Apple's AAC encoder: 2112 samples (the converter reports 0).
        case .aac: 2112
        }
    }
}
