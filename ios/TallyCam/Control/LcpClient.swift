import Foundation
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "lcp")

/// WebSocket client for the LCP device endpoint. Connects out, reconnects with
/// backoff, and pings to notice a dead link. All state lives on `queue`;
/// `send` may be called from any thread.
final class LcpClient: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    enum Event {
        case opened
        case closed(reason: String)
        case message(LcpIncoming)
    }

    /// Called on the client queue.
    var onEvent: (@Sendable (Event) -> Void)?

    private static let backoff: [TimeInterval] = [1, 2, 5, 10]
    private static let pingInterval: TimeInterval = 5
    /// Previews are dropped while this many droppable messages are in flight.
    private static let maxDroppableInFlight = 2

    private let queue = DispatchQueue(label: "lcp.client")
    private lazy var session: URLSession = {
        let operationQueue = OperationQueue()
        operationQueue.underlyingQueue = queue
        operationQueue.maxConcurrentOperationCount = 1
        return URLSession(configuration: .default, delegate: self, delegateQueue: operationQueue)
    }()

    // Owned by `queue`.
    private var url: URL?
    private var task: URLSessionWebSocketTask?
    private var isOpen = false
    private var attempt = 0
    private var reconnect: DispatchWorkItem?
    private var pingTimer: DispatchSourceTimer?
    private var pingOutstanding = false
    private var droppableInFlight = 0

    /// Connects to `url`, replacing any current connection.
    func connect(to url: URL) {
        queue.async { [self] in
            self.url = url
            attempt = 0
            closeCurrent(reason: "Reconnecting")
            open()
        }
    }

    /// The server admitted the phone on this connection: the next reconnect
    /// starts again from the shortest delay.
    func admitted() {
        queue.async { [self] in
            attempt = 0
        }
    }

    /// Closes the connection and stops reconnecting.
    func disconnect() {
        queue.async { [self] in
            url = nil
            reconnect?.cancel()
            reconnect = nil
            closeCurrent(reason: "Disconnected")
        }
    }

    /// Encodes on the calling thread. Droppable messages (previews) are
    /// skipped when the link is not keeping up.
    func send(_ message: some Encodable, droppable: Bool = false) {
        guard let text = LcpCoding.encode(message) else {
            log.error("Could not encode message")
            return
        }
        send(text: text, droppable: droppable)
    }

    /// Sends a message that is already JSON text.
    func send(text: String, droppable: Bool = false) {
        queue.async { [self] in
            guard isOpen, let task else {
                return
            }
            if droppable {
                guard droppableInFlight < Self.maxDroppableInFlight else {
                    return
                }
                droppableInFlight += 1
            }
            task.send(.string(text)) { [weak self] error in
                guard let self else {
                    return
                }
                queue.async {
                    if droppable {
                        self.droppableInFlight -= 1
                    }
                    if let error {
                        self.fail(task, reason: error.localizedDescription)
                    }
                }
            }
        }
    }

    private func open() {
        reconnect?.cancel()
        reconnect = nil
        guard let url else {
            return
        }
        log.info("Connecting to \(url.absoluteString)")
        let task = session.webSocketTask(with: url)
        task.maximumMessageSize = 1 << 20
        self.task = task
        task.resume()
    }

    private func closeCurrent(reason: String) {
        pingTimer?.cancel()
        pingTimer = nil
        if let task {
            task.cancel(with: .goingAway, reason: nil)
            self.task = nil
        }
        droppableInFlight = 0
        if isOpen {
            isOpen = false
            onEvent?(.closed(reason: reason))
        }
    }

    private func fail(_ failed: URLSessionTask, reason: String) {
        guard failed === task else {
            return
        }
        log.info("Disconnected: \(reason)")
        closeCurrent(reason: reason)
        let delay = Self.backoff[min(attempt, Self.backoff.count - 1)]
        attempt += 1
        let work = DispatchWorkItem { [weak self] in
            self?.open()
        }
        reconnect = work
        queue.asyncAfter(deadline: .now() + delay, execute: work)
    }

    private func receive(on task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            guard let self else {
                return
            }
            queue.async {
                guard task === self.task else {
                    return
                }
                switch result {
                case let .success(.string(text)):
                    if let message = LcpIncoming.parse(text) {
                        self.onEvent?(.message(message))
                    }
                    self.receive(on: task)
                case let .success(.data(data)):
                    if let text = String(data: data, encoding: .utf8), let message = LcpIncoming.parse(text) {
                        self.onEvent?(.message(message))
                    }
                    self.receive(on: task)
                case .success:
                    self.receive(on: task)
                case let .failure(error):
                    self.fail(task, reason: error.localizedDescription)
                }
            }
        }
    }

    private func startPing(on task: URLSessionWebSocketTask) {
        pingOutstanding = false
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + Self.pingInterval, repeating: Self.pingInterval)
        timer.setEventHandler { [weak self] in
            guard let self, task === self.task else {
                return
            }
            if pingOutstanding {
                fail(task, reason: "No pong within \(Int(Self.pingInterval)) s")
                return
            }
            pingOutstanding = true
            task.sendPing { [weak self] error in
                guard let self else {
                    return
                }
                queue.async {
                    guard task === self.task else {
                        return
                    }
                    self.pingOutstanding = false
                    if let error {
                        self.fail(task, reason: error.localizedDescription)
                    }
                }
            }
        }
        pingTimer = timer
        timer.resume()
    }

    // MARK: URLSessionWebSocketDelegate (called on `queue`)

    func urlSession(_: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol _: String?) {
        guard webSocketTask === task else {
            return
        }
        log.info("Connected")
        isOpen = true
        // The backoff resets only once the server admits the phone
        // (`admitted()`), so a refused phone does not reconnect every second.
        onEvent?(.opened)
        receive(on: webSocketTask)
        startPing(on: webSocketTask)
    }

    func urlSession(_: URLSession, webSocketTask: URLSessionWebSocketTask,
                    didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason _: Data?)
    {
        fail(webSocketTask, reason: "Closed by server (\(closeCode.rawValue))")
    }

    func urlSession(_: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        fail(task, reason: error?.localizedDescription ?? "Connection ended")
    }
}
