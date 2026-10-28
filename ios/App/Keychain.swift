import Foundation
import Security

/// Small secrets (the shared library's password and identifier) live in the Keychain, not in the
/// app's plain settings.
enum Keychain {
    private static let service = "io.github.noxilith.EloraNotes"

    static func read(_ key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func write(_ value: String, for key: String) {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(base as CFDictionary)
        guard !value.isEmpty else { return }
        var add = base
        add[kSecValueData as String] = Data(value.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(add as CFDictionary, nil)
    }
}

/// Where the shared library's login is kept between launches.
struct ServerStore {
    private let defaults = UserDefaults.standard

    func load() -> ServerSettings? {
        guard let host = defaults.string(forKey: "server.host"), !host.isEmpty,
              let password = Keychain.read("server.password"), !password.isEmpty
        else { return nil }
        let port = defaults.integer(forKey: "server.port")
        return ServerSettings(
            host: host,
            port: UInt16(port == 0 ? 7331 : port),
            password: password,
            fingerprint: defaults.string(forKey: "server.fingerprint"),
            displayName: defaults.string(forKey: "server.name") ?? "",
            identifier: Keychain.read("server.identifier") ?? ""
        )
    }

    func save(_ settings: ServerSettings) {
        defaults.set(settings.host, forKey: "server.host")
        defaults.set(Int(settings.port), forKey: "server.port")
        defaults.set(settings.fingerprint, forKey: "server.fingerprint")
        defaults.set(settings.displayName, forKey: "server.name")
        Keychain.write(settings.password, for: "server.password")
        Keychain.write(settings.identifier, for: "server.identifier")
    }

    func clear() {
        for key in ["server.host", "server.port", "server.fingerprint", "server.name"] {
            defaults.removeObject(forKey: key)
        }
        Keychain.write("", for: "server.password")
        Keychain.write("", for: "server.identifier")
    }
}
