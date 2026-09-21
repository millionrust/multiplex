import XCTest

@MainActor
final class EnrollmentUITests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }
    func testEnrollmentSurvivesRelaunchAndCanBeCancelled() throws {
        let app = XCUIApplication()
        app.launch()
        openEnrollment(app)
        XCTAssertTrue(app.staticTexts["No enrollment request"].waitForExistence(timeout: 10))
        app.buttons["Prepare request"].tap()
        XCTAssertTrue(app.staticTexts["Request pending"].waitForExistence(timeout: 10))
        capture("pending-portrait")
        app.buttons["Copy request"].tap()
        XCTAssertTrue(app.staticTexts["Request copied."].exists)
        app.terminate()
        app.launch()
        openEnrollment(app)
        XCTAssertTrue(app.staticTexts["Request pending"].waitForExistence(timeout: 10))
        XCUIDevice.shared.orientation = .landscapeLeft
        app.buttons["Reload request"].tap()
        XCTAssertTrue(app.staticTexts["Request pending"].waitForExistence(timeout: 10))
        capture("pending-landscape")
        XCTAssertTrue(app.buttons["Export request"].isHittable)
        XCUIDevice.shared.orientation = .portrait
        app.buttons["Share request"].tap()
        // Exercise a native activity without sending data to an external application.
        capture("share-sheet")
        let copy = app.cells["Copy"]
        XCTAssertTrue(copy.waitForExistence(timeout: 5), app.debugDescription)
        copy.tap()
        app.buttons["Export request"].tap()
        XCTAssertTrue(app.textFields["DOCPicker.filenameTextField"].waitForExistence(timeout: 15))
        capture("export-picker")
        let picker = app.otherElements["Browse View (Picker)"]
        XCTAssertTrue(picker.exists)
        if app.frame.width > 600 {
            // The tablet picker exposes its visible close glyph without a reliable button label.
            picker.coordinate(withNormalizedOffset: CGVector(dx: 0.04, dy: 0.03)).tap()
        } else {
            picker.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.03))
                .press(forDuration: 0.1, thenDragTo: picker.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85)))
        }
        let enabled = NSPredicate(format: "enabled == true AND hittable == true")
        expectation(for: enabled, evaluatedWith: app.buttons["Cancel request"])
        waitForExpectations(timeout: 10)
        XCTAssertTrue(app.staticTexts["Request pending"].exists)
        app.buttons["Export request"].tap()
        XCTAssertTrue(app.textFields["DOCPicker.filenameTextField"].waitForExistence(timeout: 15))
        app.buttons["Save"].tap()
        XCTAssertTrue(app.staticTexts["Request exported."].waitForExistence(timeout: 15))
        app.buttons["Cancel request"].tap()
        app.alerts.buttons["Keep request"].tap()
        XCTAssertTrue(app.staticTexts["Request pending"].exists)
        app.buttons["Cancel request"].tap()
        app.alerts.buttons["Confirm cancellation"].tap()
        XCTAssertTrue(app.staticTexts["No enrollment request"].waitForExistence(timeout: 10))
        capture("empty")
        app.buttons["Done"].tap()
        XCTAssertTrue(app.buttons["Device actions"].waitForExistence(timeout: 5))
        openEnrollment(app)
        XCTAssertTrue(app.staticTexts["No enrollment request"].waitForExistence(timeout: 10))
        app.typeKey(XCUIKeyboardKey.escape, modifierFlags: [])
        XCTAssertTrue(app.buttons["Device actions"].waitForExistence(timeout: 5))
    }

    private func openEnrollment(_ app: XCUIApplication) {
        app.buttons["Devices"].firstMatch.tap()
        let actions = app.buttons["Device actions"]
        XCTAssertTrue(actions.waitForExistence(timeout: 10))
        actions.tap()
        app.buttons["Enrollment"].tap()
    }

    private func capture(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
