/// The loopback address the app listens on and the keyboard connects to.
/// Used because free Apple ID sideloading grants no App Group.
enum SpikeLink {
    static let host = "127.0.0.1"
    static let port: UInt16 = 51730
}
