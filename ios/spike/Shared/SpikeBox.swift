import Foundation

/// The App Group mailbox shared by the app and the keyboard.
///
/// Free-account signing tools may rename the App Group (for example by adding
/// a team ID suffix), so the name that was actually granted is read from the
/// provisioning profile instead of assuming the requested one survived.
enum SpikeBox {
    static let requestedGroup = "group.com.siddharth189.voxspike"
    static let appFile = "from-app.txt"
    static let keyboardFile = "from-keyboard.txt"

    static func provisionedGroups() -> [String] {
        let url = Bundle.main.bundleURL.appendingPathComponent("embedded.mobileprovision")
        guard
            let data = try? Data(contentsOf: url),
            let start = data.range(of: Data("<?xml".utf8)),
            let end = data.range(of: Data("</plist>".utf8)),
            start.lowerBound < end.upperBound,
            let plist = try? PropertyListSerialization.propertyList(
                from: data.subdata(in: start.lowerBound..<end.upperBound),
                format: nil
            ) as? [String: Any],
            let entitlements = plist["Entitlements"] as? [String: Any],
            let groups = entitlements["com.apple.security.application-groups"] as? [String]
        else {
            return []
        }
        return groups
    }

    /// The first granted group whose shared folder can actually be used.
    static func container() -> (group: String, url: URL)? {
        var candidates = provisionedGroups()
        if !candidates.contains(requestedGroup) {
            candidates.append(requestedGroup)
        }
        for group in candidates {
            if let url = FileManager.default.containerURL(
                forSecurityApplicationGroupIdentifier: group
            ) {
                return (group, url)
            }
        }
        return nil
    }

    static func read(_ name: String) -> String? {
        guard let folder = container()?.url else { return nil }
        return try? String(contentsOf: folder.appendingPathComponent(name), encoding: .utf8)
    }

    /// Returns nil on success, or a message explaining why the write failed.
    static func write(_ text: String, to name: String) -> String? {
        guard let folder = container()?.url else { return "No App Group folder" }
        do {
            try text.write(
                to: folder.appendingPathComponent(name),
                atomically: true,
                encoding: .utf8
            )
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    static func diagnostics() -> String {
        let granted = provisionedGroups()
        let grantedText = granted.isEmpty ? "none found" : granted.joined(separator: ", ")
        let containerText = container()?.group ?? "UNAVAILABLE"
        return "Granted: \(grantedText)\nUsing: \(containerText)"
    }
}
