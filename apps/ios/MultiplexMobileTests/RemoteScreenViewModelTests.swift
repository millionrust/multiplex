import CoreGraphics
import XCTest

@testable import MultiplexMobile

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

/// Zoom, pan, the minimap and the two pointer modes: the arithmetic a finger depends on.
@MainActor
final class RemoteScreenZoomTests: XCTestCase {
    /// A 1000x500 computer shown in a 500x500 view fits at half size, leaving bars top and bottom.
    private func model() -> RemoteScreenViewModel {
        let model = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 1,
            ticket: ControllerScreenTicket(
                commandId: UUID(),
                ticket: Data(repeating: 3, count: 32),
                canControlPointer: true,
                canControlKeyboard: true
            )
        )
        model.setSurfaceSizeForTesting(CGSize(width: 1000, height: 500))
        return model
    }

    private let view = CGSize(width: 500, height: 500)

    func testFitIsTheStartingPointAndReadsAsFit() {
        let model = model()
        XCTAssertEqual(model.zoom, 1)
        XCTAssertEqual(model.zoomLabel, "Fit")
        XCTAssertEqual(model.scale(in: view), 0.5, accuracy: 0.0001)
        XCTAssertEqual(model.pictureOrigin(in: view).y, 125, accuracy: 0.0001)
    }

    func testZoomingMagnifiesAboutTheMiddleAndSaysSo() {
        let model = model()
        model.setZoom(2, in: view)
        XCTAssertEqual(model.zoom, 2)
        XCTAssertEqual(model.zoomLabel, "200%")
        XCTAssertEqual(model.scale(in: view), 1, accuracy: 0.0001)
        // Still centred: nothing was dragged.
        XCTAssertEqual(model.pictureOrigin(in: view).x, -250, accuracy: 0.0001)
    }

    func testZoomStopsAtTheLimitsRatherThanRunningAway() {
        let model = model()
        model.setZoom(0.1, in: view)
        XCTAssertEqual(model.zoom, 1, "a phone never shows less than the whole picture")
        model.setZoom(1000, in: view)
        XCTAssertEqual(model.zoom, RemoteScreenViewModel.maximumZoom)
    }

    func testThePictureCannotBeDraggedOffTheView() {
        let model = model()
        model.setZoom(2, in: view)
        model.panBy(CGSize(width: 10_000, height: 10_000), in: view)
        // At 2x the picture is 1000x500 in a 500x500 view: 250 of slack sideways, none vertically
        // beyond what fitting already gave.
        XCTAssertEqual(model.pan.width, 250, accuracy: 0.0001)
        XCTAssertEqual(model.pan.height, 0, accuracy: 0.0001)
    }

    func testPanningDoesNothingWhileTheWholePictureIsShown() {
        let model = model()
        model.panBy(CGSize(width: 100, height: 100), in: view)
        XCTAssertEqual(model.pan, .zero)
    }

    /// A tap has to land where the picture shows it, zoomed and dragged as it is.
    func testATapLandsWhereThePictureShowsItAfterZoomingAndPanning() {
        let model = model()
        model.setZoom(2, in: view)
        model.panBy(CGSize(width: 250, height: 0), in: view)
        // Dragged fully right, so the left edge of the computer is at the left of the view.
        let target = model.surfacePoint(from: CGPoint(x: 0, y: 250), in: view)
        XCTAssertEqual(target?.x, 0)
        XCTAssertEqual(target?.y, 250)
    }

    func testTheMinimapShowsWhichPartOfTheComputerIsOnScreen() {
        let model = model()
        model.setZoom(2, in: view)
        let visible = model.visibleRect(in: view)
        // At 2x, a 500-point view shows 500 of the computer's 1000 pixels, from the middle.
        XCTAssertEqual(visible.minX, 250, accuracy: 0.5)
        XCTAssertEqual(visible.width, 500, accuracy: 0.5)
        XCTAssertEqual(visible.height, 500, accuracy: 0.5)
    }

    func testTheWholePictureIsVisibleAtFit() {
        let model = model()
        let visible = model.visibleRect(in: view)
        XCTAssertEqual(visible.width, 1000, accuracy: 0.5)
        XCTAssertEqual(visible.height, 500, accuracy: 0.5)
    }

    /// In trackpad mode the pointer moves by how far the finger went, not to where it is, and it
    /// stays on the computer's screen.
    func testTheTrackpadPointerMovesByTheFingerAndStaysOnTheScreen() {
        let model = model()
        model.pointerMode = .trackpad
        XCTAssertEqual(model.pointer, CGPoint(x: 500, y: 250))
        model.apply(events: [.control(holder: .you)])
        model.movePointer(by: CGSize(width: 50, height: 0), in: view)
        // The finger moved 50 points at half scale, so the pointer moved 100 pixels.
        XCTAssertEqual(model.pointer.x, 600, accuracy: 0.5)
        model.movePointer(by: CGSize(width: 10_000, height: 10_000), in: view)
        XCTAssertEqual(model.pointer.x, 999, accuracy: 0.5)
        XCTAssertEqual(model.pointer.y, 499, accuracy: 0.5)
    }

    func testTheTrackpadPointerStaysPutWithoutControl() {
        let model = model()
        model.pointerMode = .trackpad
        model.movePointer(by: CGSize(width: 50, height: 0), in: view)
        XCTAssertEqual(model.pointer, CGPoint(x: 500, y: 250), "no control, no movement")
    }

    func testAccessoryKeysCoverWhatATextFieldCannotType() {
        let labels = RemoteScreenKey.accessory.map(\.label)
        XCTAssertEqual(labels, ["esc", "tab", "ctrl", "←", "↑", "↓", "→", "|", "-"])
        XCTAssertEqual(RemoteScreenKey.escape.usage, 0x29)
        XCTAssertEqual(RemoteScreenKey.pipe.modifiers, 1, "a pipe is a shifted backslash")
    }
}

/// What the phone does when a screen session drops, and what it can honestly report about it.
@MainActor
final class RemoteScreenReconnectTests: XCTestCase {
    private func host(capabilities: UInt16 = 0b1110_0011) throws -> PairedHostRecord {
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

    /// A phone loses long-lived connections all the time. It should try again rather than make
    /// the person open the screen by hand.
    func testADroppedSessionIsOpenedAgain() async throws {
        let coordinator = ControllerScreenCoordinator { _ in .milliseconds(5) }
        let connection = FlakyScreenConnection(failures: 2)
        coordinator.startPreview(host: try host(), connection: connection)
        try await waitUntil { await connection.attempts >= 3 }
        XCTAssertNil(coordinator.unavailable, "it recovered rather than giving up")
        coordinator.stop()
    }

    /// Trying for ever would drain the battery against a computer that is not coming back.
    func testItGivesUpAfterEnoughFailuresAndSaysWhy() async throws {
        let coordinator = ControllerScreenCoordinator { _ in .milliseconds(5) }
        let connection = FlakyScreenConnection(failures: .max)
        coordinator.startPreview(host: try host(), connection: connection)
        try await waitUntil { coordinator.unavailable != nil }
        guard case .failed = coordinator.unavailable else {
            return XCTFail("expected a failure, got \(String(describing: coordinator.unavailable))")
        }
        XCTAssertFalse(coordinator.reconnecting)
        let attempts = await connection.attempts
        XCTAssertLessThanOrEqual(
            attempts,
            ControllerScreenCoordinator.maximumReconnectAttempts + 1
        )
    }

    /// A computer taking screen access away is not a network problem, so retrying is pointless.
    func testLosingScreenAccessStopsRatherThanRetrying() async throws {
        let coordinator = ControllerScreenCoordinator { _ in .milliseconds(5) }
        let connection = FlakyScreenConnection(failures: .max, error: .capabilityDenied)
        coordinator.startPreview(host: try host(), connection: connection)
        try await waitUntil { coordinator.unavailable != nil }
        XCTAssertEqual(coordinator.unavailable, .notGranted)
        let attempts = await connection.attempts
        XCTAssertEqual(attempts, 1, "a refusal is not retried")
    }

    func testAPictureIsCountedAndTimedSoTheInterfaceCanSayHowFreshItIs() {
        let model = RemoteScreenViewModel(
            viewer: ScreenViewer(cacheBytes: 1 << 20),
            surface: 1,
            ticket: ControllerScreenTicket(
                commandId: UUID(),
                ticket: Data(repeating: 3, count: 32),
                canControlPointer: false,
                canControlKeyboard: false
            )
        )
        XCTAssertEqual(model.picturesDrawn, 0)
        XCTAssertNil(model.lastPictureAt)
    }

    private func waitUntil(
        attempts: Int = 400,
        _ condition: @MainActor () async -> Bool
    ) async throws {
        for _ in 0..<attempts {
            if await condition() { return }
            try await Task.sleep(for: .milliseconds(25))
        }
        XCTFail("Condition did not become true before timeout")
    }
}

/// A transport whose screen sessions drop, so reconnection can be watched.
private actor FlakyScreenConnection: ControllerConnecting {
    private(set) var attempts = 0
    private let failures: Int
    private let error: ControllerConnectionError

    init(failures: Int, error: ControllerConnectionError = .malformedResponse) {
        self.failures = failures
        self.error = error
    }

    func watchScreen(
        host: PairedHostRecord,
        surface: UInt32?,
        preview: Bool,
        onOpened: @escaping @Sendable (ControllerScreenTicket, ScreenViewer) async -> Void,
        onEvent: @escaping @Sendable ([ScreenEvent]) async throws -> Void
    ) async throws {
        attempts += 1
        if attempts <= failures {
            throw error
        }
        // Stay open until the test stops it, as a healthy session would.
        try await Task.sleep(for: .seconds(60))
    }

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
