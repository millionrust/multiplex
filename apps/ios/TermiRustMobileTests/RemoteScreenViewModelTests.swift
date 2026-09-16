import CoreGraphics
import XCTest

@testable import TermiRustMobile

/// The phone's picture of a computer's screen: what it draws, and where a tap lands.
@MainActor
final class RemoteScreenViewModelTests: XCTestCase {
    private func ticket(pointer: Bool = true, keyboard: Bool = false) -> ControllerScreenTicket {
        ControllerScreenTicket(
            commandId: UUID(),
            ticket: Data(repeating: 3, count: 32),
            canControlPointer: pointer,
            canControlKeyboard: keyboard
        )
    }

    private func model(pointer: Bool = true) -> RemoteScreenViewModel {
        RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 1,
            ticket: ticket(pointer: pointer)
        )
    }

    func testNothingIsDrawnBeforeAnyPixelsArrive() {
        let model = model()
        XCTAssertEqual(model.state, .opening)
        XCTAssertNil(model.image)
        XCTAssertEqual(model.size, .zero)
    }

    func testTheComputerEndingTheSessionIsShownRatherThanAFrozenPicture() {
        let model = model()
        model.apply(events: [.closed(reason: "sharing_stopped")])
        XCTAssertEqual(model.state, .closed(reason: "sharing_stopped"))
    }

    func testControlFollowsWhoTheComputerSaysHoldsIt() {
        let model = model()
        XCTAssertEqual(model.control, .nobody)
        model.apply(events: [.control(holder: .anotherDevice)])
        XCTAssertEqual(model.control, .anotherDevice)
        model.apply(events: [.control(holder: .you)])
        XCTAssertEqual(model.control, .you)
    }

    func testPreviewUpdatesForOtherSurfacesAreIgnored() {
        let model = model()
        model.apply(events: [
            .updated(surface: 2, preview: false, damaged: [], reset: true),
            .updated(surface: 1, preview: true, damaged: [], reset: true),
        ])
        XCTAssertEqual(model.state, .opening, "neither update was this surface's full view")
    }

    /// A tap has to land where the picture shows it, which means undoing the fit-and-centre.
    func testATapLandsWhereThePictureShowsIt() {
        let model = model()
        model.setSurfaceSizeForTesting(CGSize(width: 1000, height: 500))

        // A 400x400 view fits a 2:1 picture as 400x200, leaving 100pt bars above and below.
        let view = CGSize(width: 400, height: 400)
        let middle = model.surfacePoint(from: CGPoint(x: 200, y: 200), in: view)
        XCTAssertEqual(middle?.x, 500)
        XCTAssertEqual(middle?.y, 250)

        let topLeft = model.surfacePoint(from: CGPoint(x: 0, y: 100), in: view)
        XCTAssertEqual(topLeft?.x, 0)
        XCTAssertEqual(topLeft?.y, 0)

        // The bars are not the computer's screen.
        XCTAssertNil(model.surfacePoint(from: CGPoint(x: 200, y: 50), in: view))
        XCTAssertNil(model.surfacePoint(from: CGPoint(x: 200, y: 350), in: view))
        XCTAssertNil(model.surfacePoint(from: CGPoint(x: -1, y: 200), in: view))
    }

    func testNothingIsSentBeforeTheComputerGivesControl() {
        let watcher = model(pointer: false)
        watcher.setSurfaceSizeForTesting(CGSize(width: 100, height: 100))
        watcher.apply(events: [.control(holder: .you)])
        watcher.tap(at: CGPoint(x: 10, y: 10), in: CGSize(width: 100, height: 100))
        XCTAssertFalse(watcher.canControlPointer, "this device may not point at all")

        let granted = model()
        granted.setSurfaceSizeForTesting(CGSize(width: 100, height: 100))
        granted.tap(at: CGPoint(x: 10, y: 10), in: CGSize(width: 100, height: 100))
        XCTAssertEqual(granted.control, .nobody, "control was never given, so nothing was sent")
    }
}

/// Watching a real Mac, and the small preview a computer's page shows.
@MainActor
final class RemoteScreenSurfaceTests: XCTestCase {
    private func ticket() -> ControllerScreenTicket {
        ControllerScreenTicket(
            commandId: UUID(),
            ticket: Data(repeating: 3, count: 32),
            canControlPointer: true,
            canControlKeyboard: false
        )
    }

    private func surface(id: UInt32, name: String) -> ScreenSurface {
        ScreenSurface(id: id, width: 3024, height: 1964, scaleMilli: 2000, name: name)
    }

    /// A Mac names its displays by their own ids, so a phone that asked for "whatever you have"
    /// has to take the first one the welcome lists rather than guess.
    func testAPhoneTakesTheFirstDisplayTheComputerOffers() {
        let model = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: nil,
            ticket: ticket()
        )
        XCTAssertNil(model.displayName)
        model.apply(events: [
            .welcomed(
                surfaces: [
                    surface(id: 724_002_212, name: "Main Display"),
                    surface(id: 724_002_213, name: "Display 2"),
                ],
                resume: .notRequested
            )
        ])
        XCTAssertEqual(model.displayName, "Main Display")
    }

    /// Asking for a display the computer does not have still leaves a picture to show.
    func testAnUnknownDisplayFallsBackToTheFirstOne() {
        let model = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 1,
            ticket: ticket()
        )
        model.apply(events: [
            .welcomed(surfaces: [surface(id: 724_002_212, name: "Main Display")], resume: .notRequested)
        ])
        XCTAssertEqual(model.displayName, "Main Display")
    }

    /// The preview draws the computer's thumbnail profile, and the viewer draws the full view;
    /// each has to ignore the other's pictures or they would fight over one canvas.
    func testAPreviewDrawsThumbnailsAndAViewerDrawsTheFullView() {
        let preview = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 7,
            ticket: ticket(),
            preview: true
        )
        preview.apply(events: [.updated(surface: 7, preview: false, damaged: [], reset: true)])
        XCTAssertEqual(preview.state, .opening, "the full view is not the preview's picture")

        let viewer = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 7,
            ticket: ticket()
        )
        viewer.apply(events: [.updated(surface: 7, preview: true, damaged: [], reset: true)])
        XCTAssertEqual(viewer.state, .opening, "a thumbnail is not the viewer's picture")
    }

    /// A preview is for looking at, not for driving.
    func testAPreviewNeverSendsInput() {
        let preview = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 7,
            ticket: ticket(),
            preview: true
        )
        preview.apply(events: [.control(holder: .you)])
        preview.setSurfaceSizeForTesting(CGSize(width: 100, height: 100))
        // Nothing to assert but the absence of a crash and of queued input: the session was
        // never connected, so any send would trap on a closed viewer.
        preview.tap(at: CGPoint(x: 10, y: 10), in: CGSize(width: 100, height: 100))
        XCTAssertEqual(preview.control, .you)
    }
}

/// Which computers the phone may watch, and what it says when it may not.
@MainActor
final class ControllerScreenCoordinatorTests: XCTestCase {
    private func host(capabilities: UInt16) throws -> PairedHostRecord {
        try PairedHostRecord(
            id: "host-1",
            displayName: "Office Mac",
            route: HostRoute(address: "192.168.1.10", port: 63322),
            hostStaticPublicKey: Data(repeating: 1, count: 32),
            deviceStaticKeyId: "key-1",
            deviceId: UUID(),
            identityGeneration: 1,
            revocationEpoch: 1,
            sessionGeneration: 1,
            capabilityBits: capabilities,
            pairedAt: Date(timeIntervalSince1970: 1)
        )
    }

    func testOnlyAComputerThatGrantedScreenAccessMayBeWatched() throws {
        XCTAssertFalse(ControllerScreenCoordinator.mayWatch(try host(capabilities: 0b11)))
        XCTAssertTrue(ControllerScreenCoordinator.mayWatch(try host(capabilities: 0b10_0011)))
    }

    func testWatchingAComputerThatNeverGrantedItSaysSoInsteadOfConnecting() throws {
        let coordinator = ControllerScreenCoordinator()
        let connection = ScreenRefusingConnection()
        coordinator.startPreview(host: try host(capabilities: 0b11), connection: connection)
        XCTAssertEqual(coordinator.unavailable, .notGranted)
        XCTAssertFalse(coordinator.isWatching, "no connection was opened")
        XCTAssertNil(coordinator.preview)
    }
}

/// A transport that carries no screens, which is the protocol's default.
private actor ScreenRefusingConnection: ControllerConnecting {
    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        throw ControllerConnectionError.capabilityDenied
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        throw ControllerConnectionError.capabilityDenied
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        throw ControllerConnectionError.capabilityDenied
    }

    func requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {}

    func releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID
    ) async throws {}

    func sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        bytes: Data
    ) async throws {}

    func sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandID: UUID,
        viewport: TerminalViewportState
    ) async throws {}

    func forgetDeviceSecret(host: PairedHostRecord) async throws {}

    func cancel() async {}
}
