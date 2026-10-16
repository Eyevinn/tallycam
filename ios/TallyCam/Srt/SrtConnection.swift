import Foundation
import os
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "srt")

/// The core's SRT caller (tallycam-srt): its own thread owns the UDP socket,
/// paces, retransmits and encrypts. Every method must be called on `queue`;
/// callbacks are delivered on it too.
final class SrtConnection {
    enum Event {
        /// `latencyMs`: what both sides agreed to.
        case connected(latencyMs: Int)
        /// `reconnecting`: the caller connects again by itself. A failure
        /// before the first connection always ends the session.
        case disconnected(reason: String, reconnecting: Bool)
    }

    /// The first (`last` false) or last SRT packet of a video frame went
    /// out. Called on the core's SRT thread, twice per frame: keep it short.
    typealias FrameSent = @Sendable (_ captureNs: Int64, _ enqueuedNs: Int64, _ sentNs: Int64, _ last: Bool) -> Void

    var onEvent: ((Event) -> Void)?

    private let queue: DispatchQueue
    private let options: SrtCallerOptions
    private let frameSent: FrameSent
    private var caller: SrtCaller?
    /// `caller`'s hot handle (for `lc_ts_new`), 0 when there is none.
    private(set) var hotHandle: UInt64 = 0
    private var wasConnected = false
    private var closed = false

    init(options: SrtCallerOptions, queue: DispatchQueue, frameSent: @escaping FrameSent) {
        self.options = options
        self.queue = queue
        self.frameSent = frameSent
    }

    var stats: SrtStats? {
        caller?.stats()
    }

    func open() {
        let encryption = options.passphrase == nil ? "off" : "AES-\(Int(options.pbkeylen == 0 ? 16 : options.pbkeylen) * 8)"
        log.info("Connecting to \(self.options.host):\(self.options.port) streamid=\(self.options.streamId ?? "-") latency=\(self.options.latencyMs) ms encryption=\(encryption)")
        do {
            let caller = try SrtCaller.start(options: options,
                                          listener: Listener(connection: self, queue: queue, frameSent: frameSent))
            self.caller = caller
            hotHandle = caller.hotHandle()
        } catch let SrtError.Config(message) {
            end(message)
        } catch {
            end(error.localizedDescription)
        }
    }

    /// Sends SRT SHUTDOWN to the listener and stops the caller's thread.
    func close() {
        guard !closed else {
            return
        }
        closed = true
        hotHandle = 0
        let caller = caller
        self.caller = nil
        caller?.close()
    }

    fileprivate func received(_ event: SrtEvent) {
        guard !closed else {
            return
        }
        switch event {
        case let .connecting(attempt):
            if attempt > 1 {
                log.info("Reconnecting (attempt \(attempt))")
            }
        case let .connected(latencyMs, peer):
            wasConnected = true
            log.info("Connected to \(peer), latency \(latencyMs) ms")
            onEvent?(.connected(latencyMs: Int(latencyMs)))
        case let .disconnected(reason, reconnecting):
            guard reconnecting, wasConnected else {
                end(reason)
                return
            }
            log.info("Disconnected: \(reason), reconnecting")
            onEvent?(.disconnected(reason: reason, reconnecting: true))
        }
    }

    private func end(_ reason: String) {
        guard !closed else {
            return
        }
        close()
        onEvent?(.disconnected(reason: reason, reconnecting: false))
    }
}

/// The core calls this on its SRT thread. Events hop to the connection's
/// queue; frame-sent reports go straight to their handler, without a hop
/// per packet report. Holds the connection weakly: the core keeps the
/// listener until the caller is closed.
private final class Listener: SrtListener, @unchecked Sendable {
    private weak var connection: SrtConnection?
    private let queue: DispatchQueue
    private let frameSent: SrtConnection.FrameSent

    init(connection: SrtConnection, queue: DispatchQueue, frameSent: @escaping SrtConnection.FrameSent) {
        self.connection = connection
        self.queue = queue
        self.frameSent = frameSent
    }

    func onEvent(event: SrtEvent) {
        queue.async { [weak connection = self.connection] in
            connection?.received(event)
        }
    }

    func onFrameSent(captureNs: Int64, enqueuedNs: Int64, sentNs: Int64, last: Bool) {
        frameSent(captureNs, enqueuedNs, sentNs, last)
    }
}
