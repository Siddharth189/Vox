/// The loopback link between the keyboard and the app. Used because free
/// Apple ID sideloading grants no App Group.
///
/// The keyboard opens one connection per command and reads one reply:
/// `ok`, `text:<words>`, `empty`, or `err:<message>`.
enum SpikeLink {
    static let host = "127.0.0.1"
    static let port: UInt16 = 51730

    static let start = "start"
    static let stop = "stop"
    static let cancel = "cancel"
}
