import CoreGraphics
import Foundation

/// Why this phone is not showing a computer's screen.
enum ControllerScreenUnavailable: Equatable, Sendable {
    /// The computer never gave this device screen access.
    case notGranted
    /// The computer is not sharing its screen at all; no grant makes a picture appear.
    case sharingOff
    /// The session ended and reopening it did not help.
    case failed(String)
}

/// Owns the phone's one screen session: the small preview on a computer's page, and the full
/// viewer it opens into.
///
/// A phone holds one Controller connection at a time, so a preview and a viewer are the same
/// session in two shapes, and starting either ends the other. The last picture of each computer
/// is kept after its session ends, so Fleet can show what a computer looked like without holding
/// a connection open for every computer at once.
@MainActor
final class ControllerScreenCoordinator: ObservableObject {
    /// The small thumbnail on a computer's page, about one picture a second.
    @Published private(set) var preview: RemoteScreenViewModel?
    /// The full-size screen, once someone opens it.
    @Published private(set) var viewer: RemoteScreenViewModel?
    /// The last picture seen for each computer, keyed by host id.
    @Published private(set) var lastPictures: [String: CGImage] = [:]
    @Published private(set) var unavailable: ControllerScreenUnavailable?
    /// Set while a dropped session is being opened again. The last picture stays on screen.
    @Published private(set) var reconnecting = false
    /// How many times in a row this session has been reopened without succeeding.
    @Published private(set) var reconnectAttempt = 0

    /// The capability bit a computer grants before this phone may watch it at all.
    static let observeScreensCapability: UInt16 = 1 << 5

    private var session: Task<Void, Never>?
    private var watchingHost: PairedHostRecord?
    /// How long to wait before opening a dropped session again. A test shortens it.
    private let backoff: @Sendable (Int) -> Duration

    init(backoff: @escaping @Sendable (Int) -> Duration = ControllerScreenCoordinator.backoff) {
        self.backoff = backoff
    }

    deinit {
        session?.cancel()
    }

    /// Whether `host` has given this phone screen access.
    static func mayWatch(_ host: PairedHostRecord) -> Bool {
        host.capabilityBits & observeScreensCapability == observeScreensCapability
    }

    var isWatching: Bool { session != nil }

    /// Which session is the current one.
    ///
    /// Replacing a session does not stop the old one at once: its read fails a moment later, and
    /// its failure handler used to run against whatever was current by then — clearing the viewer
    /// that had just opened. Every callback carries the token of the session it belongs to and is
    /// ignored when that is no longer this one.
    private var generation = 0

    /// Starts the one-picture-a-second preview for a computer's page.
    func startPreview(host: PairedHostRecord, connection: any ControllerConnecting) {
        start(host: host, connection: connection, preview: true)
    }

    /// Opens the full screen, on `surface` when the person picked one of the computer's displays.
    /// The preview, if any, ends: there is one connection.
    func openViewer(
        host: PairedHostRecord,
        connection: any ControllerConnecting,
        surface: UInt32? = nil
    ) {
        start(host: host, connection: connection, preview: false, surface: surface)
    }

    /// Closes the viewer and goes back to previewing the same computer.
    func closeViewer(connection: (any ControllerConnecting)?) {
        guard let host = watchingHost, let connection else {
            stop()
            return
        }
        startPreview(host: host, connection: connection)
    }

    /// Ends whatever session is running and keeps the last picture.
    func stop() {
        session?.cancel()
        session = nil
        watchingHost = nil
        preview = nil
        viewer = nil
        reconnecting = false
        reconnectAttempt = 0
    }

    private func start(
        host: PairedHostRecord,
        connection: any ControllerConnecting,
        preview wantsPreview: Bool,
        surface: UInt32? = nil
    ) {
        guard Self.mayWatch(host) else {
            unavailable = .notGranted
            return
        }
        session?.cancel()
        session = nil
        self.preview = nil
        viewer = nil
        unavailable = nil
        reconnecting = false
        reconnectAttempt = 0
        watchingHost = host
        generation += 1
        run(
            host: host,
            connection: connection,
            preview: wantsPreview,
            surface: surface,
            token: generation
        )
    }

    /// Opens the session, and opens it again from the last picture when it drops.
    ///
    /// A screen session is a long-lived connection, and a phone loses those: it changes network,
    /// sleeps, or walks out of range. The last picture stays on screen while this works, because
    /// a frozen picture of the right computer is more use than an empty one.
    private func run(
        host: PairedHostRecord,
        connection: any ControllerConnecting,
        preview wantsPreview: Bool,
        surface: UInt32? = nil,
        token: Int
    ) {
        let hostID = host.id
        session = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    try await connection.watchScreen(
                        host: host,
                        surface: surface,
                        preview: wantsPreview,
                        onOpened: { [weak self] ticket, viewer in
                            await self?.opened(
                                ticket: ticket,
                                viewer: viewer,
                                preview: wantsPreview,
                                token: token
                            )
                        },
                        onEvent: { [weak self] events in
                            await self?.apply(events: events, hostID: hostID, token: token)
                        }
                    )
                    return
                } catch is CancellationError {
                    return
                } catch {
                    guard let self else { return }
                    let keepTrying = await self.dropped(error, token: token)
                    guard keepTrying else { return }
                }
                guard let self else { return }
                let wait = await self.backoff(self.reconnectAttempt)
                try? await Task.sleep(for: wait)
            }
        }
    }

    /// How long to wait before opening the session again, growing with each failure.
    static let backoff: @Sendable (Int) -> Duration = { attempt in
        .milliseconds(min(500 * (1 << max(attempt - 1, 0)), 8_000))
    }

    /// After this many failures in a row the phone stops and says so, rather than draining the
    /// battery against a computer that is not coming back.
    static let maximumReconnectAttempts = 5

    /// What the listener answers `OpenScreen` with when a computer shares no screen.
    static let sharingOffCode = "screen_sharing_off"

    /// Records a dropped session. Returns whether it is worth opening again.
    private func dropped(_ error: Error, token: Int) -> Bool {
        // A session that has already been replaced does not get to answer for the one that
        // replaced it.
        guard token == generation else { return false }
        // The computer answered by name: it is not sharing its screen. Nothing on this phone
        // changes that, and asking again gets the same answer, so it is said rather than
        // retried behind a spinner that never resolves.
        if let error = error as? ControllerConnectionError,
           case .hostError(let code) = error, code == Self.sharingOffCode {
            unavailable = .sharingOff
            reconnecting = false
            preview = nil
            viewer = nil
            return false
        }
        if let error = error as? ControllerConnectionError, error == .capabilityDenied {
            // The computer took screen access away; trying again would only be refused.
            unavailable = .notGranted
            reconnecting = false
            preview = nil
            viewer = nil
            return false
        }
        reconnectAttempt += 1
        guard reconnectAttempt <= Self.maximumReconnectAttempts else {
            unavailable = .failed(Self.message(for: error))
            reconnecting = false
            preview = nil
            viewer = nil
            return false
        }
        // The models stay, so the last picture stays on screen while this reconnects.
        reconnecting = true
        return true
    }

    private func opened(
        ticket: ControllerScreenTicket,
        viewer screenViewer: ScreenViewer,
        preview wantsPreview: Bool,
        token: Int
    ) {
        guard token == generation else { return }
        let model = RemoteScreenViewModel(
            viewer: screenViewer,
            surface: nil,
            ticket: ticket,
            preview: wantsPreview
        )
        if wantsPreview {
            preview = model
        } else {
            viewer = model
        }
    }

    private func apply(events: [ScreenEvent], hostID: String, token: Int) {
        guard token == generation else { return }
        let model = viewer ?? preview
        model?.apply(events: events)
        if let picture = model?.image {
            lastPictures[hostID] = picture
            // Pictures are arriving again, so the session is back.
            reconnecting = false
            reconnectAttempt = 0
        }
    }

    private static func message(for error: Error) -> String {
        guard let error = error as? ControllerConnectionError else {
            return "The screen session stopped."
        }
        switch error {
        case .capabilityDenied:
            return "This computer has not given this phone screen access."
        default:
            return "The screen session stopped."
        }
    }
}
