import XCTest

final class BoopmarkUITests: XCTestCase {
    private func launchApp(additionalArguments: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["--uitesting"] + additionalArguments
        app.launch()
        return app
    }

    func testBookmarkHomeLoads() {
        let app = launchApp()

        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Add bookmark"].exists || app.buttons["Settings"].exists)
    }

    func testAppWindowUsesFullDeviceViewport() {
        let app = launchApp()
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 5))

        let screenSize = XCUIScreen.main.screenshot().image.size
        let windowSize = window.frame.size
        let screenAspectRatio = screenSize.width / screenSize.height
        let windowAspectRatio = windowSize.width / windowSize.height

        XCTAssertEqual(
            windowAspectRatio,
            screenAspectRatio,
            accuracy: 0.03,
            "The app window should fill the device rather than run in a letterboxed compatibility viewport."
        )
    }

    func testCreateAppStoreScreenshots() {
        let app = launchApp(additionalArguments: ["--appstore-screenshots"])
        XCTAssertTrue(app.staticTexts["Swift: A powerful language for every platform"]
            .waitForExistence(timeout: 5))
        addAppStoreScreenshot(named: "01-organize-everything", app: app)

        app.buttons["capture.toolbarButton"].tap()
        XCTAssertTrue(app.navigationBars["Save bookmark"].waitForExistence(timeout: 3))
        addAppStoreScreenshot(named: "02-save-in-seconds", app: app)
        app.buttons["Cancel"].tap()

        app.buttons["bookmark.row.10000000-0000-0000-0000-000000000001"].tap()
        XCTAssertTrue(app.navigationBars["Bookmark"].waitForExistence(timeout: 3))
        addAppStoreScreenshot(named: "03-notes-and-tags", app: app)
    }

    private func addAppStoreScreenshot(named name: String, app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testSettingsPresentsConnectionAndOfflineQueueControls() {
        let app = launchApp()
        app.buttons["settings.button"].tap()

        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 2))
        XCTAssertTrue(app.textFields["settings.serverURL"].exists)
        XCTAssertTrue(app.secureTextFields["settings.apiKey"].exists)
        app.swipeUp()
        XCTAssertTrue(app.staticTexts["Waiting to send"].waitForExistence(timeout: 2))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 2))
    }

    func testSortAndTagFilterControls() {
        let app = launchApp()
        app.buttons["bookmarks.filters"].tap()
        app.buttons["Filter by tags"].tap()

        XCTAssertTrue(app.navigationBars["Sort and filter"].waitForExistence(timeout: 2))
        let tags = app.textFields["bookmarks.filterTags"]
        XCTAssertTrue(tags.exists)
        tags.tap()
        tags.typeText("ios, swift")
        app.navigationBars["Sort and filter"].tap()
        app.buttons["bookmarks.applyFilters"].tap()
        XCTAssertFalse(app.navigationBars["Sort and filter"].exists)
    }

    func testCapturePreservesAQueryBearingURL() {
        let app = launchApp()
        app.buttons["capture.toolbarButton"].tap()

        XCTAssertTrue(app.navigationBars["Save bookmark"].waitForExistence(timeout: 2))
        let urlField = app.textFields["capture.url"]
        XCTAssertTrue(urlField.exists)
        urlField.tap()
        urlField.typeText("https://example.com/article?id=42#comments")
        app.navigationBars["Save bookmark"].tap()
        app.swipeUp()
        XCTAssertTrue(app.buttons["capture.save"].waitForExistence(timeout: 2))
        XCTAssertTrue(app.buttons["capture.save"].isEnabled)
        app.buttons["Cancel"].tap()
    }

    /// Opt-in helper for exercising a deployed server with the production
    /// settings and Keychain stores. Credentials are supplied by the caller
    /// and are never committed to the project or printed by the test.
    func testProvisionLiveServerConnection() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let serverURL = environment["BOOPMARK_LIVE_SERVER_URL"],
              let apiKey = environment["BOOPMARK_LIVE_API_KEY"],
              !serverURL.isEmpty,
              !apiKey.isEmpty else {
            throw XCTSkip("Set BOOPMARK_LIVE_SERVER_URL and BOOPMARK_LIVE_API_KEY to provision a live connection.")
        }

        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))
        app.buttons["settings.button"].tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 3))

        let serverField = app.textFields["settings.serverURL"]
        serverField.tap()
        serverField.press(forDuration: 1)
        if app.menuItems["Select All"].waitForExistence(timeout: 2) {
            app.menuItems["Select All"].tap()
        }
        serverField.typeText(serverURL)

        let keyField = app.secureTextFields["settings.apiKey"]
        keyField.tap()
        keyField.typeText(apiKey)
        app.buttons["Save connection"].tap()

        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.alerts["Boopmark"].exists)
    }

    func testLiveProductionCapture() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to mutate the configured live server.")
        }
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))
        app.buttons["capture.toolbarButton"].tap()
        XCTAssertTrue(app.navigationBars["Save bookmark"].waitForExistence(timeout: 3))
        let urlField = app.textFields["https://…"]
        urlField.tap()
        urlField.typeText("https://example.com/?boopmark-ios-production-e2e=20260822")
        app.buttons["Save bookmark"].tap()
        let captureDismissed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: app.navigationBars["Save bookmark"]
        )
        XCTAssertEqual(XCTWaiter.wait(for: [captureDismissed], timeout: 90), .completed)
        XCTAssertFalse(app.alerts["Boopmark"].exists)

        app.terminate()
        app.launch()
        let enrichedTitle = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH %@", "Example Domain")
        ).firstMatch
        XCTAssertTrue(enrichedTitle.waitForExistence(timeout: 20))
    }

    func testLiveProductionLLMAutofillParity() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to mutate the configured live server.")
        }
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.buttons["capture.toolbarButton"].waitForExistence(timeout: 5))
        app.buttons["capture.toolbarButton"].tap()
        XCTAssertTrue(app.navigationBars["Save bookmark"].waitForExistence(timeout: 3))

        let url = app.descendants(matching: .any)["capture.url"]
        url.tap()
        url.typeText("https://www.rfc-editor.org/rfc/rfc9110.html?boopmark-ios-llm-autofill-e2e=20260823")
        app.buttons["capture.autofill"].tap()
        XCTAssertTrue(
            app.staticTexts["AI filled title, note, and tags."].waitForExistence(timeout: 90)
        )
        let autofillScreenshot = XCTAttachment(screenshot: app.screenshot())
        autofillScreenshot.name = "Production Anthropic capture autofill"
        autofillScreenshot.lifetime = .keepAlways
        add(autofillScreenshot)
        app.buttons["Cancel"].tap()
        let search = app.searchFields.firstMatch
        search.tap()
        search.typeText("Condensing 20 Years")
        app.keyboards.buttons["search"].tap()
        let existingTitle = app.staticTexts[
            "Condensing 20 Years of PostgreSQL Knowledge Into a Single Markdown"
        ].firstMatch
        XCTAssertTrue(existingTitle.waitForExistence(timeout: 20))
        existingTitle.tap()
        XCTAssertTrue(app.navigationBars["Bookmark"].waitForExistence(timeout: 5))

        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Edit"].tap()
        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["sparkles"].tap()
        XCTAssertTrue(
            app.staticTexts["AI filled title, note, and tags."].waitForExistence(timeout: 90)
        )
        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Cancel editing"].tap()
    }

    func testLiveProductionSortAndTagFilterParity() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to use the configured live server.")
        }
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))

        let expectedTitle = "Condensing 20 Years of PostgreSQL Knowledge Into a Single Markdown"
        let search = app.searchFields.firstMatch
        search.tap()
        search.typeText("Condensing 20 Years")
        app.keyboards.buttons["search"].tap()
        let bookmark = app.staticTexts[expectedTitle].firstMatch
        XCTAssertTrue(bookmark.waitForExistence(timeout: 20))
        bookmark.tap()

        let tag = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bookmark.tag."))
            .firstMatch
        XCTAssertTrue(tag.waitForExistence(timeout: 5))
        let tagName = tag.label
        XCTAssertFalse(tagName.isEmpty)
        app.navigationBars["Bookmark"].buttons.firstMatch.tap()

        app.buttons["Clear text"].tap()
        app.buttons["close"].tap()
        XCTAssertTrue(app.buttons["bookmarks.filters"].waitForExistence(timeout: 3))
        app.buttons["bookmarks.filters"].tap()
        app.buttons["Filter by tags"].tap()
        let tags = app.textFields["bookmarks.filterTags"]
        tags.tap()
        tags.typeText(tagName)
        app.swipeUp()
        XCTAssertTrue(app.buttons["Title"].waitForExistence(timeout: 3))
        app.buttons["Title"].tap()
        app.buttons["bookmarks.applyFilters"].tap()

        XCTAssertTrue(app.staticTexts[expectedTitle].firstMatch.waitForExistence(timeout: 20))
    }

    func testLiveProductionEditSearchAndDelete() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to mutate the configured live server.")
        }
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))

        let search = app.searchFields.firstMatch
        search.tap()
        search.typeText("Example Domain")
        search.press(forDuration: 0.1)
        app.keyboards.buttons["search"].tap()
        let enrichedTitle = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH %@", "Example Domain")
        ).firstMatch
        XCTAssertTrue(enrichedTitle.waitForExistence(timeout: 10))
        enrichedTitle.tap()
        XCTAssertTrue(app.navigationBars["Bookmark"].waitForExistence(timeout: 5))

        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Edit"].tap()
        let title = app.textFields["bookmark.edit.title"]
        title.tap()
        title.press(forDuration: 1)
        if app.menuItems["Select All"].waitForExistence(timeout: 2) { app.menuItems["Select All"].tap() }
        title.typeText("Boopmark iOS Production E2E Edited")
        let tags = app.textFields["bookmark.edit.tags"]
        tags.tap()
        tags.press(forDuration: 1)
        if app.menuItems["Select All"].waitForExistence(timeout: 2) { app.menuItems["Select All"].tap() }
        tags.typeText("ios-e2e, production-test")
        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Save changes"].tap()
        let editFinished = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: app.textFields["bookmark.edit.title"]
        )
        XCTAssertEqual(XCTWaiter.wait(for: [editFinished], timeout: 15), .completed)

        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Delete"].tap()
        XCTAssertTrue(app.buttons["Delete bookmark"].waitForExistence(timeout: 3))
        app.buttons["Delete bookmark"].tap()
        let detailDismissed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: app.buttons["bookmark.detailMenu"]
        )
        XCTAssertEqual(XCTWaiter.wait(for: [detailDismissed], timeout: 15), .completed)
    }

    @available(iOS 16.4, *)
    func testLiveProductionSafariShareCapture() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to mutate the configured live server.")
        }
        let fixtureID = UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(10)
        let fixtureTitle = "Boopmark native share E2E \(fixtureID)"
        let sharedURL = "https://example.com/?boopmark-native-share-e2e=\(fixtureID)"
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.buttons["settings.button"].waitForExistence(timeout: 5))
        if app.staticTexts["Connect your server in Settings to get started."].exists {
            throw XCTSkip("Provision the dedicated live-test simulator before exercising production sharing.")
        }

        let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        safari.open(URL(string: sharedURL)!)
        XCTAssertTrue(safari.wait(for: .runningForeground, timeout: 5))
        if safari.buttons["Close"].exists { safari.buttons["Close"].tap() }
        XCTAssertTrue(safari.staticTexts["Example Domain"].waitForExistence(timeout: 5))
        safari.buttons["More"].tap()
        XCTAssertTrue(safari.buttons["Share"].waitForExistence(timeout: 3))
        safari.buttons["Share"].tap()
        let boopmarkAction = safari.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", "Boopmark"))
            .firstMatch
        XCTAssertTrue(boopmarkAction.waitForExistence(timeout: 5))
        boopmarkAction.tap()

        let shareExtension = XCUIApplication(bundleIdentifier: "com.boopmark.ios.share")
        XCTAssertTrue(shareExtension.navigationBars["Save to Boopmark"].waitForExistence(timeout: 5))
        XCTAssertTrue(shareExtension.staticTexts[sharedURL].exists)
        let title = shareExtension.textFields["share.title"]
        XCTAssertTrue(title.exists)
        title.tap()
        title.typeText(fixtureTitle)
        shareExtension.navigationBars["Save to Boopmark"].tap()
        shareExtension.buttons["share.save"].tap()

        let extensionDismissed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: shareExtension.navigationBars["Save to Boopmark"]
        )
        XCTAssertEqual(XCTWaiter.wait(for: [extensionDismissed], timeout: 90), .completed)
        app.activate()

        // The main app keeps its own in-memory list while the Share Extension
        // writes through a separate process. Returning to the foreground must
        // reconcile that list without requiring a force quit or manual search.
        let sharedBookmark = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS %@", String(fixtureID))
        ).firstMatch
        XCTAssertTrue(sharedBookmark.waitForExistence(timeout: 30))
        sharedBookmark.tap()
        XCTAssertTrue(app.navigationBars["Bookmark"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts[sharedURL].exists)

        app.buttons["bookmark.detailMenu"].tap()
        app.buttons["Delete"].tap()
        XCTAssertTrue(app.buttons["Delete bookmark"].waitForExistence(timeout: 3))
        app.buttons["Delete bookmark"].tap()
        let detailDismissed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: app.buttons["bookmark.detailMenu"]
        )
        XCTAssertEqual(XCTWaiter.wait(for: [detailDismissed], timeout: 15), .completed)
    }

    @available(iOS 16.4, *)
    func testLiveProductionShareAutofillsBeforeSave() throws {
        guard ProcessInfo.processInfo.environment["BOOPMARK_RUN_LIVE_E2E"] == "1" else {
            throw XCTSkip("Set BOOPMARK_RUN_LIVE_E2E=1 to use the configured live server.")
        }
        let sharedURL = "https://www.dbreunig.com/2026/08/14/harnesses-are-situated-agents.html"
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.buttons["settings.button"].waitForExistence(timeout: 5))

        let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        safari.open(URL(string: sharedURL)!)
        XCTAssertTrue(safari.wait(for: .runningForeground, timeout: 5))
        if safari.buttons["Close"].exists { safari.buttons["Close"].tap() }
        XCTAssertTrue(safari.buttons["More"].waitForExistence(timeout: 10))
        safari.buttons["More"].tap()
        XCTAssertTrue(safari.buttons["Share"].waitForExistence(timeout: 3))
        safari.buttons["Share"].tap()
        let boopmarkAction = safari.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", "Boopmark"))
            .firstMatch
        XCTAssertTrue(boopmarkAction.waitForExistence(timeout: 5))
        boopmarkAction.tap()

        let shareExtension = XCUIApplication(bundleIdentifier: "com.boopmark.ios.share")
        XCTAssertTrue(shareExtension.navigationBars["Save to Boopmark"].waitForExistence(timeout: 5))
        XCTAssertTrue(shareExtension.staticTexts[sharedURL].exists)
        XCTAssertTrue(shareExtension.staticTexts["share.autofillStatus"].waitForExistence(timeout: 90))

        for identifier in ["share.title", "share.note", "share.tags"] {
            let field = shareExtension.textFields[identifier]
            XCTAssertTrue(field.exists)
            XCTAssertFalse((field.value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
        }
        shareExtension.buttons["Cancel"].tap()
        app.activate()
    }

    @available(iOS 16.4, *)
    func testSafariShareSheetOpensBoopmarkExtension() throws {
        let app = launchApp(additionalArguments: ["--share-e2e"])
        XCTAssertTrue(app.navigationBars["Boopmark"].waitForExistence(timeout: 5))

        app.buttons["settings.button"].tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 3))
        app.swipeUp()
        while app.buttons["Remove from queue"].firstMatch.exists {
            app.buttons["Remove from queue"].firstMatch.tap()
        }
        app.buttons["Done"].tap()

        let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        safari.open(URL(string: "https://example.com/?boopmark-share-e2e=1")!)
        XCTAssertTrue(safari.wait(for: .runningForeground, timeout: 5))

        if safari.buttons["Close"].exists { safari.buttons["Close"].tap() }
        let moreButton = safari.buttons["More"]
        XCTAssertTrue(moreButton.waitForExistence(timeout: 5))
        moreButton.tap()

        let shareButton = safari.buttons["Share"]
        XCTAssertTrue(shareButton.waitForExistence(timeout: 3))
        shareButton.tap()

        let boopmarkAction = safari.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", "Boopmark"))
            .firstMatch
        XCTAssertTrue(boopmarkAction.waitForExistence(timeout: 5))
        boopmarkAction.tap()

        let shareExtension = XCUIApplication(bundleIdentifier: "com.boopmark.ios.share")
        XCTAssertTrue(shareExtension.navigationBars["Save to Boopmark"].waitForExistence(timeout: 5))
        XCTAssertTrue(shareExtension.staticTexts["https://example.com/?boopmark-share-e2e=1"].exists)
        XCTAssertTrue(shareExtension.buttons["share.autofill"].exists)
        let screenshot = XCTAttachment(screenshot: shareExtension.screenshot())
        screenshot.name = "Boopmark Share Extension"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        shareExtension.buttons["Save bookmark"].tap()

        app.activate()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 5))
        XCTAssertTrue(app.buttons["settings.button"].waitForExistence(timeout: 5))
        app.buttons["settings.button"].tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 5))
        app.swipeUp()
        XCTAssertEqual(app.staticTexts["settings.pendingCount"].label, "1")
        XCTAssertTrue(app.descendants(matching: .any)["settings.pending.https://example.com/?boopmark-share-e2e=1"].exists)
        app.buttons["Remove from queue"].firstMatch.tap()
        XCTAssertEqual(app.staticTexts["settings.pendingCount"].label, "0")
    }
}
