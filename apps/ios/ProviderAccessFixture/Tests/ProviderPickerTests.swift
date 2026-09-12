import XCTest

@MainActor
final class ProviderPickerTests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }
    func testExternalPickerReadAndReselection() {
        let donor = XCUIApplication(bundleIdentifier: "com.termirust.fixture.documents")
        donor.launch()
        XCTAssertTrue(donor.staticTexts["Fixture ready"].waitForExistence(timeout: 10))
        donor.terminate()
        let app = XCUIApplication()
        app.launch()
        selectDocument(app)
        XCTAssertTrue(app.staticTexts["Verified external scoped read"].waitForExistence(timeout: 15), app.debugDescription)
        let image = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        image.name = "external-scoped-read"
        image.lifetime = .keepAlways
        add(image)
        app.buttons["Select fixture"].tap()
        let cancel = app.navigationBars.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancel.waitForExistence(timeout: 10), app.debugDescription)
        cancel.tap()
        XCTAssertTrue(app.staticTexts["Cancelled"].waitForExistence(timeout: 10))
        app.terminate()
        app.launch()
        XCTAssertTrue(app.staticTexts["Idle"].waitForExistence(timeout: 10))
        selectDocument(app)
        XCTAssertTrue(app.staticTexts["Verified external scoped read"].waitForExistence(timeout: 15), app.debugDescription)
    }

    private func selectDocument(_ app: XCUIApplication) {
        app.buttons["Select fixture"].tap()
        let browse = app.tabBars.buttons["Browse"].firstMatch
        if browse.waitForExistence(timeout: 3) { browse.tap() }
        let local = app.cells.matching(NSPredicate(format: "label CONTAINS[c] 'On My iPhone'")).firstMatch
        if local.waitForExistence(timeout: 5) { local.tap() }
        let folder = app.cells.matching(NSPredicate(format: "label CONTAINS 'C06Documents'")).firstMatch
        if folder.waitForExistence(timeout: 5) { folder.tap() }
        let document = app.cells.matching(NSPredicate(format: "label CONTAINS 'c06-transfer'")).firstMatch
        XCTAssertTrue(document.waitForExistence(timeout: 10), app.debugDescription)
        let picker = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        picker.name = "external-file-picker"
        picker.lifetime = .keepAlways
        add(picker)
        // Target the thumbnail, not the combined icon/filename/metadata cell center.
        document.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2)).tap()
    }
}
