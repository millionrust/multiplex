import CoreGraphics
import Foundation

/// Why this phone is not showing a computer's screen.
enum ControllerScreenUnavailable: Equatable, Sendable {
    /// The computer never gave this device screen access.
    case notGranted
    /// The computer is sharing nothing, or the session ended.
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

    /// The capability bit a computer grants before this phone may watch it at all.
    static let observeScreensCapability: UInt16 = 1 << 5

    private var session: Task<Void, Never>?
    private var watchingHost: PairedHostRecord?

    deinit {
        session?.cancel()
    }

    /// Whether `host` has given this phone screen access.
    static func mayWatch(_ host: PairedHostRecord) -> Bool {
        host.capabilityBits & observeScreensCapability == observeScreensCapability
    }

    var isWatching: Bool { session != nil }

    /// Starts the one-picture-a-second preview for a computer's page.
    func startPreview(host: PairedHostRecord, connection: any ControllerConnecting) {
        start(host: host, connection: connection, preview: true)
    }

    /// Opens the full screen. The preview, if any, ends: there is one connection.
    func openViewer(host: PairedHostRecord, connection: any ControllerConnecting) {
        start(host: host, connection: connection, preview: false)
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
    }

    private func start(
        host: PairedHostRecord,
        connection: any ControllerConnecting,
        preview wantsPreview: Bool
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
        watchingHost = host
        let hostID = host.id
        session = Task { [weak self] in
            do {
                try await connection.watchScreen(
                    host: host,
                    surface: nil,
                    preview: wantsPreview,
                    onOpened: { [weak self] ticket, viewer in
                        await self?.opened(
                            ticket: ticket,
                            viewer: viewer,
                            preview: wantsPreview
                        )
                    },
                    onEvent: { [weak self] events in
                        await self?.apply(events: events, hostID: hostID)
                    }
                )
            } catch is CancellationError {
                return
            } catch {
                await self?.failed(error)
            }
        }
    }

    private func opened(
        ticket: ControllerScreenTicket,
        viewer screenViewer: ScreenViewer,
        preview wantsPreview: Bool
    ) {
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

    private func apply(events: [ScreenEvent], hostID: String) {
        let model = viewer ?? preview
        model?.apply(events: events)
        if let picture = model?.image {
            lastPictures[hostID] = picture
        }
    }

    private func failed(_ error: Error) {
        guard !Task.isCancelled else { return }
        session = nil
        preview = nil
        viewer = nil
        unavailable = .failed(Self.message(for: error))
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
