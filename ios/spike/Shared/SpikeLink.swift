/// The loopback link between the keyboard and the app. Used because free
/// Apple ID sideloading grants no App Group.
///
/// The keyboard opens one connection per command and reads one reply:
/// `ok`, `text:<words>`, `empty`, or `err:<message>`.
enum SpikeLink {
    static let host = "127.0.0.1"
    static let port: UInt16 = 51730

    static let start = "start"
    static let cancel = "cancel"

    /// How the speech should be written. Picked on the keyboard and sent with stop.
    enum Mode: String, CaseIterable {
        /// Translate everything to English.
        case english
        /// Keep the words as spoken, Hindi in Latin letters.
        case hinglish

        var title: String {
            switch self {
            case .english: return "English"
            case .hinglish: return "Hinglish"
            }
        }
    }

    static let stopPrefix = "stop"

    static func stop(_ mode: Mode) -> String {
        "\(stopPrefix):\(mode.rawValue)"
    }

    /// The mode in a stop command, or nil if the command isn't a stop.
    /// A bare `stop` means English.
    static func stopMode(in command: String) -> Mode? {
        if command == stopPrefix {
            return .english
        }
        guard command.hasPrefix(stopPrefix + ":") else { return nil }
        return Mode(rawValue: String(command.dropFirst(stopPrefix.count + 1)))
    }
}
