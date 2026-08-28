import Foundation
import XCTest
@testable import BoopmarkShared

final class APIClientTests: XCTestCase {
    private static let testGate = DispatchSemaphore(value: 1)

    override func setUp() {
        super.setUp()
        Self.testGate.wait()
    }

    override func tearDown() {
        StubURLProtocol.active = nil
        Self.testGate.signal()
        super.tearDown()
    }

    func testListUsesBasePathQueryAndBearerToken() async throws {
        let bookmark = makeBookmark()
        let stub = StubURLProtocol()
        stub.setResponse(status: 200, data: try JSONEncoder.boopmark.encode([bookmark]))
        let api = makeClient(stub: stub)

        let result = try await api.list(
            BookmarkQuery(search: "swift rocks", tags: ["swift", "ios"], sort: .title, limit: 5, offset: 10)
        )

        XCTAssertEqual(result.count, 1)
        XCTAssertEqual(result[0].id, bookmark.id)
        XCTAssertEqual(result[0].userID, bookmark.userID)
        XCTAssertEqual(result[0].url.absoluteString, bookmark.url.absoluteString)
        XCTAssertEqual(result[0].title, bookmark.title)
        XCTAssertEqual(result[0].tags, bookmark.tags)
        XCTAssertEqual(Int(result[0].createdAt.timeIntervalSince1970), Int(bookmark.createdAt.timeIntervalSince1970))
        XCTAssertEqual(Int(result[0].updatedAt.timeIntervalSince1970), Int(bookmark.updatedAt.timeIntervalSince1970))
        let request = try XCTUnwrap(stub.lastRequest)
        XCTAssertEqual(request.httpMethod, "GET")
        XCTAssertEqual(request.url?.path, "/boopmark/api/v1/bookmarks")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-token")
        let query = try XCTUnwrap(URLComponents(url: try XCTUnwrap(request.url), resolvingAgainstBaseURL: false)?.queryItems)
        let values = Dictionary(uniqueKeysWithValues: query.map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(values["sort"], "title")
        XCTAssertEqual(values["limit"], "5")
        XCTAssertEqual(values["offset"], "10")
        XCTAssertEqual(values["search"], "swift rocks")
        XCTAssertEqual(values["tags"], "swift,ios")
    }

    func testCreateSendsSuggestedQueryAndSnakeCaseBody() async throws {
        let stub = StubURLProtocol()
        stub.setResponse(status: 201, data: try JSONEncoder.boopmark.encode(makeBookmark()))
        let api = makeClient(stub: stub)
        let input = CreateBookmark(
            url: URL(string: "https://example.com/articles/swift")!,
            title: "A title",
            description: "A note",
            imageURL: URL(string: "https://example.com/image.png"),
            domain: "example.com",
            tags: ["swift", "mobile"]
        )

        _ = try await api.create(input, suggest: true)

        let request = try XCTUnwrap(stub.lastRequest)
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.url?.path, "/boopmark/api/v1/bookmarks")
        XCTAssertEqual(URLComponents(url: try XCTUnwrap(request.url), resolvingAgainstBaseURL: false)?.queryItems?.first?.value, "true")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/json")
        let body = try XCTUnwrap(stub.lastBody)
        let object = try XCTUnwrap(try JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertEqual(object["url"] as? String, input.url.absoluteString)
        XCTAssertEqual(object["image_url"] as? String, input.imageURL?.absoluteString)
        XCTAssertEqual(object["domain"] as? String, "example.com")
        XCTAssertEqual(object["tags"] as? [String], ["swift", "mobile"])
    }

    func testSuggestAndUpdateUseExpectedPathsAndBodies() async throws {
        let stub = StubURLProtocol()
        stub.setResponse(status: 200, data: try JSONEncoder.boopmark.encode(makeBookmark()))
        let api = makeClient(stub: stub)
        let id = UUID(uuidString: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")!

        _ = try await api.suggest(url: URL(string: "https://example.com/source")!)
        var request = try XCTUnwrap(stub.lastRequest)
        XCTAssertEqual(request.url?.path, "/boopmark/api/v1/bookmarks/suggest")
        XCTAssertEqual(request.httpMethod, "POST")
        let suggestBody = try XCTUnwrap(stub.lastBody)
        XCTAssertEqual((try JSONSerialization.jsonObject(with: suggestBody) as? [String: String])?["url"], "https://example.com/source")

        _ = try await api.update(id: id, input: UpdateBookmark(title: "Updated", tags: ["one"]), suggest: true)
        request = try XCTUnwrap(stub.lastRequest)
        XCTAssertEqual(request.url?.path, "/boopmark/api/v1/bookmarks/\(id.uuidString)")
        XCTAssertEqual(URLComponents(url: try XCTUnwrap(request.url), resolvingAgainstBaseURL: false)?.queryItems?.first?.name, "suggest")
        let updateBody = try XCTUnwrap(stub.lastBody)
        let updateObject = try XCTUnwrap(try JSONSerialization.jsonObject(with: updateBody) as? [String: Any])
        XCTAssertEqual(updateObject["title"] as? String, "Updated")
        XCTAssertEqual(updateObject["tags"] as? [String], ["one"])
    }

    func testDeleteAcceptsNoContent() async throws {
        let stub = StubURLProtocol()
        stub.setResponse(status: 204, data: Data())
        let api = makeClient(stub: stub)
        let id = UUID(uuidString: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")!

        try await api.delete(id: id)

        let request = try XCTUnwrap(stub.lastRequest)
        XCTAssertEqual(request.httpMethod, "DELETE")
        XCTAssertEqual(request.url?.path, "/boopmark/api/v1/bookmarks/\(id.uuidString)")
    }

    func testUnauthorizedAndConflictMapToActionableErrors() async throws {
        let stub = StubURLProtocol()
        let api = makeClient(stub: stub)

        stub.setResponse(status: 401, data: Data(#"{"error":"unauthorized"}"#.utf8))
        do {
            _ = try await api.list()
            XCTFail("Expected unauthorized error")
        } catch let error as BoopmarkAPIError {
            XCTAssertEqual(error, .unauthorized)
        }

        stub.setResponse(status: 409, data: Data(#"{"error":"already exists"}"#.utf8))
        do {
            _ = try await api.create(CreateBookmark(url: URL(string: "https://example.com")!), suggest: false)
            XCTFail("Expected conflict error")
        } catch let error as BoopmarkAPIError {
            XCTAssertEqual(error, .conflict)
        }
    }

    func testValidationStatusPreservesServerErrorMessageAndRejectsInsecureBaseURL() async throws {
        let stub = StubURLProtocol()
        stub.setResponse(status: 422, data: Data(#"{"error":"invalid input"}"#.utf8))
        let api = makeClient(stub: stub)
        do {
            _ = try await api.list()
            XCTFail("Expected validation error")
        } catch let error as BoopmarkAPIError {
            XCTAssertEqual(error, .invalidResponse(422, "invalid input"))
        }

        let insecure = BoopmarkAPI(baseURL: URL(string: "http://example.com")!, token: "secret")
        do {
            _ = try await insecure.list()
            XCTFail("Expected invalid URL error")
        } catch let error as BoopmarkAPIError {
            XCTAssertEqual(error, .invalidURL)
        }
    }

    private func makeClient(stub: StubURLProtocol) -> BoopmarkAPI {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        StubURLProtocol.active = stub
        return BoopmarkAPI(
            baseURL: URL(string: "https://example.com/boopmark")!,
            token: "test-token",
            session: URLSession(configuration: configuration)
        )
    }

    private func makeBookmark() -> Bookmark {
        Bookmark(
            id: UUID(uuidString: "11111111-1111-1111-1111-111111111111")!,
            userID: UUID(uuidString: "22222222-2222-2222-2222-222222222222")!,
            url: URL(string: "https://example.com/articles/swift")!,
            title: "Swift on the go",
            description: "A useful article",
            imageURL: URL(string: "https://example.com/image.png"),
            domain: "example.com",
            tags: ["swift", "mobile"],
            createdAt: Date(timeIntervalSince1970: 1_787_400_000.123),
            updatedAt: Date(timeIntervalSince1970: 1_787_400_001.123)
        )
    }
}

private struct StubResponse: Sendable {
    let status: Int
    let data: Data
}

private final class StubURLProtocol: URLProtocol {
    static var active: StubURLProtocol?
    private var response = StubResponse(status: 500, data: Data())
    private(set) var lastRequest: URLRequest?
    private(set) var lastBody: Data?

    func setResponse(status: Int, data: Data) { response = StubResponse(status: status, data: data) }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let active = Self.active else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        active.lastRequest = request
        active.lastBody = request.httpBody ?? Self.readBodyStream(request.httpBodyStream)
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: active.response.status,
            httpVersion: nil,
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: active.response.data)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func readBodyStream(_ stream: InputStream?) -> Data? {
        guard let stream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count < 0 { return nil }
            if count == 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}
