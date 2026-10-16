import Accelerate
import Atomics
@preconcurrency import AVFoundation
import AudioToolbox
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "mic")

/// The microphone, outside AVCaptureSession: a RemoteIO audio unit (no voice
/// processing) on an audio session in measurement mode with 5 ms I/O
/// buffers at 48 kHz, mono Float32.
///
/// The I/O thread only renders into a preallocated slot of a lock-free ring
/// (no locks, no allocation) and wakes the delivery thread, which stamps
/// each buffer (`AudioTiming.captureNs`) and hands it to the sink: the audio
/// encoder while live.
///
/// Interruptions (a call, Siri) stop the unit and restart it when they end;
/// route changes re-read the input latency (it is part of the timestamp);
/// a media services reset rebuilds the unit. The built-in microphone is the
/// preferred input, so a wired headset does not take over the camera's
/// sound; Bluetooth (HFP) input is not allowed.
final class MicCapture: @unchecked Sendable {
    /// Mono samples, their first sample's capture time and the time the
    /// buffer reached the delivery thread (host ns), and the sample rate.
    typealias Sink = (_ samples: UnsafeBufferPointer<Float>, _ captureNs: Int64,
                      _ arrivedNs: Int64, _ sampleRate: Double) -> Void

    static let sampleRate: Double = 48000
    static let ioBufferDuration: TimeInterval = 0.005

    /// Peak and RMS for the operator's meter.
    let levels = MicLevels()

    private let ring = MicRing()
    private let lock = NSLock()
    // Guarded by `lock`.
    private var sink: Sink?
    private var inputLatency: TimeInterval = 0
    // Main thread.
    private var unit: AudioUnit?
    private var observers: [NSObjectProtocol] = []
    private var thread: Thread?
    private var started = false
    private var lastSampleTime: Double?

    deinit {
        stopUnit()
        // Lets the delivery thread see that this is gone and end.
        ring.wakeup.signal()
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    func setSink(_ sink: Sink?) {
        lock.withLock {
            self.sink = sink
        }
    }

    /// Configures the audio session and starts the microphone. Call once,
    /// after microphone access was granted. Returns false when there is no
    /// usable microphone.
    @MainActor
    func start() -> Bool {
        guard !started else {
            return unit != nil
        }
        started = true
        configureSession()
        observeSession()
        startThread()
        return startUnit()
    }

    // MARK: Audio session

    @MainActor
    private func configureSession() {
        let session = AVAudioSession.sharedInstance()
        do {
            // Measurement: no voice processing, no automatic gain.
            try session.setCategory(.playAndRecord, mode: .measurement, options: [.defaultToSpeaker])
            try session.setPreferredSampleRate(Self.sampleRate)
            try session.setPreferredIOBufferDuration(Self.ioBufferDuration)
            preferBuiltInMic(session)
            try session.setActive(true)
        } catch {
            log.error("Audio session setup failed: \(error.localizedDescription)")
        }
        // Only valid on an active session; the unit converts to mono anyway.
        do {
            try session.setPreferredInputNumberOfChannels(1)
        } catch {
            log.info("Preferred input channels not set: \(error.localizedDescription)")
        }
        readRoute()
    }

    @MainActor
    private func preferBuiltInMic(_ session: AVAudioSession) {
        guard let builtIn = session.availableInputs?.first(where: { $0.portType == .builtInMic }),
              session.preferredInput?.uid != builtIn.uid
        else {
            return
        }
        do {
            try session.setPreferredInput(builtIn)
        } catch {
            log.error("Built-in microphone not selectable: \(error.localizedDescription)")
        }
    }

    /// The input latency is part of every timestamp; it changes with the route.
    @MainActor
    private func readRoute() {
        let session = AVAudioSession.sharedInstance()
        let latency = session.inputLatency
        lock.withLock {
            inputLatency = latency
        }
        let input = session.currentRoute.inputs.first.map { "\($0.portName) (\($0.portType.rawValue))" } ?? "none"
        log.info("Microphone \(input, privacy: .public): \(session.sampleRate) Hz, I/O buffer \(session.ioBufferDuration * 1000) ms, input latency \(latency * 1000) ms, \(session.inputNumberOfChannels) ch")
    }

    @MainActor
    private func observeSession() {
        let center = NotificationCenter.default
        let session = AVAudioSession.sharedInstance()
        observers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: session,
                                            queue: .main)
            { [weak self] note in
                let type = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt)
                    .flatMap(AVAudioSession.InterruptionType.init(rawValue:))
                MainActor.assumeIsolated {
                    self?.interrupted(type)
                }
            })
        observers.append(center.addObserver(forName: AVAudioSession.routeChangeNotification, object: session,
                                            queue: .main)
            { [weak self] note in
                let reason = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt)
                    .flatMap(AVAudioSession.RouteChangeReason.init(rawValue:))
                MainActor.assumeIsolated {
                    self?.routeChanged(reason)
                }
            })
        observers.append(center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification,
                                            object: session, queue: .main)
            { [weak self] _ in
                MainActor.assumeIsolated {
                    log.info("Media services were reset, rebuilding the microphone")
                    self?.stopUnit()
                    self?.configureSession()
                    _ = self?.startUnit()
                }
            })
    }

    @MainActor
    private func interrupted(_ type: AVAudioSession.InterruptionType?) {
        switch type {
        case .began:
            log.info("Microphone interrupted")
            if let unit {
                AudioOutputUnitStop(unit)
            }
        case .ended:
            log.info("Microphone interruption ended")
            do {
                try AVAudioSession.sharedInstance().setActive(true)
            } catch {
                log.error("Audio session not reactivated: \(error.localizedDescription)")
            }
            readRoute()
            if let unit {
                let status = AudioOutputUnitStart(unit)
                if status != noErr {
                    log.error("Microphone restart failed: \(status)")
                }
            }
        default:
            break
        }
    }

    @MainActor
    private func routeChanged(_ reason: AVAudioSession.RouteChangeReason?) {
        log.info("Audio route changed (\(reason.map { String($0.rawValue) } ?? "?"))")
        let session = AVAudioSession.sharedInstance()
        if reason == .newDeviceAvailable || reason == .oldDeviceUnavailable {
            preferBuiltInMic(session)
        }
        readRoute()
        // The unit stops when the hardware format changes under it.
        if let unit, !Self.isRunning(unit) {
            let status = AudioOutputUnitStart(unit)
            log.info("Microphone restarted after the route change: \(status)")
        }
    }

    // MARK: Audio unit

    @MainActor
    private func startUnit() -> Bool {
        var description = AudioComponentDescription(componentType: kAudioUnitType_Output,
                                                    componentSubType: kAudioUnitSubType_RemoteIO,
                                                    componentManufacturer: kAudioUnitManufacturer_Apple,
                                                    componentFlags: 0, componentFlagsMask: 0)
        guard let component = AudioComponentFindNext(nil, &description) else {
            log.error("No RemoteIO unit")
            return false
        }
        var newUnit: AudioUnit?
        guard AudioComponentInstanceNew(component, &newUnit) == noErr, let unit = newUnit else {
            log.error("RemoteIO unit not created")
            return false
        }
        var on: UInt32 = 1
        var off: UInt32 = 0
        var format = AudioStreamBasicDescription(
            mSampleRate: Self.sampleRate, mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved,
            mBytesPerPacket: 4, mFramesPerPacket: 1, mBytesPerFrame: 4, mChannelsPerFrame: 1,
            mBitsPerChannel: 32, mReserved: 0
        )
        var maxFrames = UInt32(MicRing.slotFrames)
        var callback = AURenderCallbackStruct(inputProc: micInputCallback,
                                              inputProcRefCon: Unmanaged.passUnretained(ring).toOpaque())
        let uint32Size = UInt32(MemoryLayout<UInt32>.size)
        let steps: [(String, OSStatus)] = [
            ("enable input", AudioUnitSetProperty(unit, kAudioOutputUnitProperty_EnableIO,
                                                  kAudioUnitScope_Input, 1, &on, uint32Size)),
            ("disable output", AudioUnitSetProperty(unit, kAudioOutputUnitProperty_EnableIO,
                                                    kAudioUnitScope_Output, 0, &off, uint32Size)),
            // The client side of the input bus: the unit converts from the
            // hardware's rate and channels if they differ.
            ("format", AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 1,
                                            &format, UInt32(MemoryLayout<AudioStreamBasicDescription>.size))),
            ("max frames", AudioUnitSetProperty(unit, kAudioUnitProperty_MaximumFramesPerSlice,
                                                kAudioUnitScope_Global, 0, &maxFrames, uint32Size)),
            ("input callback", AudioUnitSetProperty(unit, kAudioOutputUnitProperty_SetInputCallback,
                                                    kAudioUnitScope_Global, 0, &callback,
                                                    UInt32(MemoryLayout<AURenderCallbackStruct>.size))),
        ]
        if let failed = steps.first(where: { $0.1 != noErr }) {
            log.error("RemoteIO \(failed.0, privacy: .public) failed: \(failed.1)")
            AudioComponentInstanceDispose(unit)
            return false
        }
        ring.unit = unit
        var status = AudioUnitInitialize(unit)
        if status == noErr {
            status = AudioOutputUnitStart(unit)
        }
        guard status == noErr else {
            log.error("RemoteIO start failed: \(status)")
            ring.unit = nil
            AudioComponentInstanceDispose(unit)
            return false
        }
        self.unit = unit
        log.info("Microphone started (RemoteIO)")
        return true
    }

    private func stopUnit() {
        guard let unit else {
            return
        }
        AudioOutputUnitStop(unit)
        AudioUnitUninitialize(unit)
        AudioComponentInstanceDispose(unit)
        ring.unit = nil
        self.unit = nil
    }

    private static func isRunning(_ unit: AudioUnit) -> Bool {
        var running: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        AudioUnitGetProperty(unit, kAudioOutputUnitProperty_IsRunning, kAudioUnitScope_Global, 0, &running, &size)
        return running != 0
    }

    // MARK: Delivery

    private func startThread() {
        let thread = Thread { [weak self, ring] in
            while true {
                ring.wakeup.wait()
                guard let self else {
                    return
                }
                deliver()
            }
        }
        thread.name = "mic.delivery"
        thread.qualityOfService = .userInteractive
        self.thread = thread
        thread.start()
    }

    /// On the delivery thread: stamps and hands over every filled slot.
    private func deliver() {
        while let slot = ring.nextFilled() {
            let arrivedNs = HostClock.nowNs()
            let (sink, latency) = lock.withLock { (self.sink, self.inputLatency) }
            if ring.overflows.exchange(0, ordering: .relaxed) > 0 {
                log.error("Microphone buffers dropped: the delivery thread fell behind")
            }
            let hostNs = slot.hostTicks != 0
                ? HostClock.ns(fromTicks: slot.hostTicks)
                : arrivedNs - AudioTiming.durationNs(frames: slot.frames, sampleRate: Self.sampleRate)
            let captureNs = AudioTiming.captureNs(bufferHostNs: hostNs, inputLatency: latency)
            if let last = lastSampleTime, slot.sampleTime != last {
                log.info("Microphone sample time jumped by \(slot.sampleTime - last) samples")
            }
            lastSampleTime = slot.sampleTime + Double(slot.frames)
            let samples = UnsafeBufferPointer(start: slot.samples, count: slot.frames)
            levels.measure(samples, sampleRate: Self.sampleRate)
            sink?(samples, captureNs, arrivedNs, Self.sampleRate)
            ring.release()
        }
    }
}

/// The microphone level for the operator's meter: measured on the delivery
/// thread (never the I/O thread) with vDSP, read on the main thread through
/// atomics.
final class MicLevels: @unchecked Sendable {
    /// Time constant of the RMS reading.
    private static let rmsWindow = 0.3

    /// Highest peak since the last `read`, as Float bits.
    private let peakBits = ManagedAtomic<UInt32>(0)
    private let meanSquareBits = ManagedAtomic<UInt32>(0)
    // Delivery thread only.
    private var meanSquare: Float = 0

    /// On the delivery thread, once per buffer.
    func measure(_ samples: UnsafeBufferPointer<Float>, sampleRate: Double) {
        guard let base = samples.baseAddress, !samples.isEmpty else {
            return
        }
        let count = vDSP_Length(samples.count)
        var peak: Float = 0
        vDSP_maxmgv(base, 1, &peak, count)
        var bufferMeanSquare: Float = 0
        vDSP_measqv(base, 1, &bufferMeanSquare, count)
        guard peak.isFinite, bufferMeanSquare.isFinite else {
            return
        }
        let alpha = Float(min(1, Double(samples.count) / (sampleRate * Self.rmsWindow)))
        meanSquare += alpha * (bufferMeanSquare - meanSquare)
        meanSquareBits.store(meanSquare.bitPattern, ordering: .relaxed)
        var old = peakBits.load(ordering: .relaxed)
        while peak > Float(bitPattern: old) {
            let (exchanged, original) = peakBits.compareExchange(expected: old, desired: peak.bitPattern,
                                                                 ordering: .relaxed)
            if exchanged {
                break
            }
            old = original
        }
    }

    /// The highest peak since the last read and the current RMS, both
    /// linear (1 = full scale).
    func read() -> (peak: Float, rms: Float) {
        let peak = Float(bitPattern: peakBits.exchange(0, ordering: .relaxed))
        let rms = Float(bitPattern: meanSquareBits.load(ordering: .relaxed)).squareRoot()
        return (peak, rms)
    }
}

/// Single-producer (I/O thread), single-consumer (delivery thread) ring of
/// preallocated sample slots. The producer never blocks, allocates or locks.
final class MicRing: @unchecked Sendable {
    static let slotCount = 32
    /// Largest I/O buffer accepted (MaximumFramesPerSlice).
    static let slotFrames = 4096

    struct Slot {
        let samples: UnsafeMutablePointer<Float>
        var frames = 0
        /// `AudioTimeStamp.mHostTime` of the first sample, 0 if not valid.
        var hostTicks: UInt64 = 0
        var sampleTime: Double = 0
    }

    /// Set while the unit runs; read on the I/O thread.
    var unit: AudioUnit?
    let wakeup = DispatchSemaphore(value: 0)
    let overflows = ManagedAtomic<Int>(0)
    private let slots: UnsafeMutablePointer<Slot>
    private let bufferList: UnsafeMutableAudioBufferListPointer
    /// Slots written (producer) and read (consumer), both only grow.
    private let written = ManagedAtomic<Int>(0)
    private let read = ManagedAtomic<Int>(0)

    init() {
        slots = .allocate(capacity: Self.slotCount)
        for i in 0 ..< Self.slotCount {
            let samples = UnsafeMutablePointer<Float>.allocate(capacity: Self.slotFrames)
            samples.initialize(repeating: 0, count: Self.slotFrames)
            (slots + i).initialize(to: Slot(samples: samples))
        }
        bufferList = AudioBufferList.allocate(maximumBuffers: 1)
    }

    deinit {
        for i in 0 ..< Self.slotCount {
            slots[i].samples.deallocate()
        }
        slots.deinitialize(count: Self.slotCount)
        slots.deallocate()
        free(bufferList.unsafeMutablePointer)
    }

    /// On the I/O thread.
    func render(_ flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>,
                _ timeStamp: UnsafePointer<AudioTimeStamp>, _ bus: UInt32, _ frames: UInt32) -> OSStatus
    {
        guard let unit else {
            return noErr
        }
        let w = written.load(ordering: .relaxed)
        guard w - read.load(ordering: .acquiring) < Self.slotCount, Int(frames) <= Self.slotFrames else {
            overflows.wrappingIncrement(ordering: .relaxed)
            return noErr
        }
        let slot = slots + w % Self.slotCount
        bufferList[0] = AudioBuffer(mNumberChannels: 1, mDataByteSize: frames * 4,
                                    mData: UnsafeMutableRawPointer(slot.pointee.samples))
        let status = AudioUnitRender(unit, flags, timeStamp, bus, frames, bufferList.unsafeMutablePointer)
        guard status == noErr else {
            return status
        }
        let stamp = timeStamp.pointee
        slot.pointee.frames = Int(frames)
        slot.pointee.hostTicks = stamp.mFlags.contains(.hostTimeValid) ? stamp.mHostTime : 0
        slot.pointee.sampleTime = stamp.mSampleTime
        written.store(w + 1, ordering: .releasing)
        wakeup.signal()
        return noErr
    }

    /// On the delivery thread: the oldest filled slot, valid until `release`.
    func nextFilled() -> Slot? {
        let r = read.load(ordering: .relaxed)
        guard r < written.load(ordering: .acquiring) else {
            return nil
        }
        return slots[r % Self.slotCount]
    }

    func release() {
        read.wrappingIncrement(ordering: .releasing)
    }
}

private func micInputCallback(_ refCon: UnsafeMutableRawPointer,
                              _ flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>,
                              _ timeStamp: UnsafePointer<AudioTimeStamp>,
                              _ bus: UInt32,
                              _ frames: UInt32,
                              _: UnsafeMutablePointer<AudioBufferList>?) -> OSStatus
{
    Unmanaged<MicRing>.fromOpaque(refCon).takeUnretainedValue().render(flags, timeStamp, bus, frames)
}
