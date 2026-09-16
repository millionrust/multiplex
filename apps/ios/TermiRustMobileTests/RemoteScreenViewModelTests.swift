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
