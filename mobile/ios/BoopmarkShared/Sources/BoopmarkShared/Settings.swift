import Foundation

public struct BoopmarkSettings: Sendable, Equatable {
    public var serverURL: URL?
    public var apiKey: String?

    public init(serverURL: URL? = nil, apiKey: String? = nil) {
        self.serverURL = serverURL
        self.apiKey = apiKey
    }

    public var isConfigured: Bool {
        serverURL != nil && !(apiKey?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }
}

public enum SettingsStoreError: Error, LocalizedError, Equatable, Sendable {
    case verificationFailed
    case sharedContainerUnavailable

    public var errorDescription: String? {
        switch self {
        case .verificationFailed: return "Boopmark could not verify the saved connection."
        case .sharedContainerUnavailable: return "Boopmark cannot access its shared app data. Check App Group signing."
        }
    }
}

public enum ServerURLValidationError: Error, LocalizedError, Equatable, Sendable {
    case missingHost
    case unsupportedScheme
    case credentialsNotAllowed

    public var errorDescription: String? {
        switch self {
        case .missingHost: return "Enter a server URL with a hostname."
        case .unsupportedScheme: return "Use HTTPS for your Boopmark server (HTTP is only allowed for localhost)."
        case .credentialsNotAllowed: return "Server URLs cannot contain a username or password."
        }
    }
}

public enum ServerURLValidator {
    /// Normalizes the value users paste into Settings while refusing URLs that
    /// could unexpectedly send the API key to an insecure or credentialed host.
    /// Local HTTP is intentionally allowed for the project's local development
    /// server; production URLs must use HTTPS.
    public static func normalize(_ rawValue: String) throws -> URL {
        var raw = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if !raw.contains("://") { raw = "https://\(raw)" }
        guard var components = URLComponents(string: raw),
              let host = components.host,
              !host.isEmpty else { throw ServerURLValidationError.missingHost }
        if components.user != nil || components.password != nil {
            throw ServerURLValidationError.credentialsNotAllowed
        }
        let isLocal = host.caseInsensitiveCompare("localhost") == .orderedSame
            || host == "127.0.0.1"
            || host == "::1"
            || host == "[::1]"
        guard components.scheme?.lowercased() == "https"
            || (components.scheme?.lowercased() == "http" && isLocal)
        else { throw ServerURLValidationError.unsupportedScheme }
        components.scheme = components.scheme?.lowercased()
        let path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = path.isEmpty ? "" : "/\(path)"
        components.query = nil
        components.fragment = nil
        guard let url = components.url else { throw ServerURLValidationError.missingHost }
        return url
    }
}

/// Validates a bookmark URL without changing the resource it identifies.
/// Unlike server-base normalization, query strings and fragments are kept.
public enum BookmarkURLValidator {
    public static func validate(_ rawValue: String) throws -> URL {
        let raw = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let components = URLComponents(string: raw),
              let scheme = components.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              let host = components.host,
              !host.isEmpty else { throw ServerURLValidationError.missingHost }
        if components.user != nil || components.password != nil {
            throw ServerURLValidationError.credentialsNotAllowed
        }
        guard let url = components.url else { throw ServerURLValidationError.missingHost }
        return url
    }
}

public protocol SettingsStore: Sendable {
    func load() throws -> BoopmarkSettings
    func saveServerURL(_ url: URL?) throws
    func saveAPIKey(_ key: String?) throws
}

/// UserDefaults holds only the non-secret server address. API keys go through
/// the Keychain on Apple platforms; the in-memory fallback exists so the
/// shared package remains testable on non-Apple Swift toolchains.
public final class UserDefaultsSettingsStore: @unchecked Sendable, SettingsStore {
    private let defaults: UserDefaults?
    private let keychain: TokenStore

    public init(
        suiteName: String = "group.com.boopmark.shared",
        keychain: TokenStore? = nil
    ) {
        self.defaults = UserDefaults(suiteName: suiteName)
        let accessGroup = Bundle.main.object(forInfoDictionaryKey: "BoopmarkKeychainAccessGroup") as? String
        self.keychain = keychain ?? KeychainTokenStore(
            service: "com.boopmark.api-key",
            accessGroup: accessGroup
        )
    }

    public func load() throws -> BoopmarkSettings {
        guard let defaults else { throw SettingsStoreError.sharedContainerUnavailable }
        let serverURL = defaults.string(forKey: "serverURL").flatMap { try? ServerURLValidator.normalize($0) }
        return BoopmarkSettings(serverURL: serverURL, apiKey: try keychain.read())
    }

    public func saveServerURL(_ url: URL?) throws {
        guard let defaults else { throw SettingsStoreError.sharedContainerUnavailable }
        guard let url else {
            defaults.removeObject(forKey: "serverURL")
            return
        }
        let normalized = try ServerURLValidator.normalize(url.absoluteString)
        defaults.set(normalized.absoluteString, forKey: "serverURL")
    }

    public func saveAPIKey(_ key: String?) throws {
        if let key, !key.isEmpty { try keychain.save(key) } else { try keychain.delete() }
    }
}

public protocol TokenStore: Sendable {
    func read() throws -> String?
    func save(_ token: String) throws
    func delete() throws
}

public enum TokenStoreError: Error, LocalizedError, Equatable, Sendable {
    case security(Int32)
    case invalidData

    public var errorDescription: String? {
        switch self {
        case let .security(status): return "Keychain operation failed (status \(status))."
        case .invalidData: return "The saved API key is unreadable."
        }
    }
}

#if canImport(Security)
import Security

public final class KeychainTokenStore: @unchecked Sendable, TokenStore {
    private let service: String
    private let account: String
    private let accessGroup: String?

    public init(service: String, account: String = "default", accessGroup: String? = nil) {
        self.service = service
        self.account = account
        self.accessGroup = accessGroup
    }

    public func read() throws -> String? {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne
        ]
        if let accessGroup { query[kSecAttrAccessGroup] = accessGroup }
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw TokenStoreError.security(status) }
        guard let data = result as? Data else { throw TokenStoreError.invalidData }
        return String(data: data, encoding: .utf8)
    }

    public func save(_ token: String) throws {
        let data = Data(token.utf8)
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account
        ]
        if let accessGroup { query[kSecAttrAccessGroup] = accessGroup }
        let attributes: [CFString: Any] = [kSecValueData: data]
        let updateStatus = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecItemNotFound {
            var add = query
            add[kSecValueData] = data
            let addStatus = SecItemAdd(add as CFDictionary, nil)
            guard addStatus == errSecSuccess else { throw TokenStoreError.security(addStatus) }
        } else if updateStatus != errSecSuccess {
            throw TokenStoreError.security(updateStatus)
        }
    }

    public func delete() throws {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account
        ]
        if let accessGroup { query[kSecAttrAccessGroup] = accessGroup }
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw TokenStoreError.security(status)
        }
    }
}
#else
public final class KeychainTokenStore: @unchecked Sendable, TokenStore {
    private var value: String?
    public init(service: String, account: String = "default", accessGroup: String? = nil) {}
    public func read() throws -> String? { value }
    public func save(_ token: String) throws { value = token }
    public func delete() throws { value = nil }
}
#endif
