import AVFoundation
import Network
import SwiftUI

@main
struct VoxSpikeApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

/// Keeps the microphone running so iOS keeps the app alive in the background.
/// Audio is only counted, never stored or sent anywhere.
final class MicSession: ObservableObject {
    @Published private(set) var status = "Off"
    @Published private(set) var buffers = 0

    private let engine = AVAudioEngine()

    var isRunning: Bool { engine.isRunning }

    func start() {
        AVAudioApplication.requestRecordPermission { granted in
            DispatchQueue.main.async {
                if granted {
                    self.begin()
                } else {
                    self.status = "Microphone permission denied"
                }
            }
        }
    }

    private func begin() {
        guard !engine.isRunning else { return }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, options: [.mixWithOthers])
            try session.setActive(true)

            let input = engine.inputNode
            input.removeTap(onBus: 0)
            input.installTap(
                onBus: 0,
                bufferSize: 4096,
                format: input.outputFormat(forBus: 0)
            ) { [weak self] _, _ in
                DispatchQueue.main.async { self?.buffers += 1 }
            }
            engine.prepare()
            try engine.start()
            status = "Running"
        } catch {
            status = "Failed: \(error.localizedDescription)"
        }
    }
}

final class LinkServer: ObservableObject {
    @Published private(set) var state = "Not started"
    @Published private(set) var foregroundPings = 0
    @Published private(set) var backgroundPings = 0

    private var listener: NWListener?

    /// Set by the view so replies can report the microphone's progress.
    var micReport: () -> String = { "" }

    func start() {
        guard listener == nil, let port = NWEndpoint.Port(rawValue: SpikeLink.port) else {
            return
        }
        do {
            let parameters = NWParameters.tcp
            parameters.allowLocalEndpointReuse = true
            parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: port)

            let listener = try NWListener(using: parameters)
            listener.stateUpdateHandler = { [weak self] newState in
                self?.update(newState)
            }
            listener.newConnectionHandler = { [weak self] connection in
                self?.handle(connection)
            }
            self.listener = listener
            listener.start(queue: .main)
        } catch {
            state = "Failed: \(error.localizedDescription)"
        }
    }

    private func update(_ newState: NWListener.State) {
        switch newState {
        case .ready:
            state = "Listening"
        case .failed(let error):
            state = "Failed: \(error.localizedDescription)"
            listener?.cancel()
            listener = nil
        case .cancelled:
            state = "Stopped"
        default:
            break
        }
    }

    private func handle(_ connection: NWConnection) {
        connection.start(queue: .main)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64) {
            [weak self] _, _, _, _ in
            guard let self else { return }
            let inBackground = UIApplication.shared.applicationState == .background
            if inBackground {
                self.backgroundPings += 1
            } else {
                self.foregroundPings += 1
            }
            let mode = inBackground ? "background" : "foreground"
            let reply = "app=\(mode) \(self.micReport())"
            connection.send(
                content: Data(reply.utf8),
                completion: .contentProcessed { _ in connection.cancel() }
            )
        }
    }
}

struct ContentView: View {
    @StateObject private var mic = MicSession()
    @StateObject private var server = LinkServer()
    @State private var typed = ""

    var body: some View {
        Form {
            Section("1. Start") {
                Button("Start mic session") { mic.start() }
                Text("Mic: \(mic.status), buffers: \(mic.buffers)")
                Text("Link: \(server.state)")
            }
            .font(.footnote.monospaced())
            Section("2. Switch to Vox Spike Keyboard, tap Run test") {
                TextField("Tap here, then use the globe key", text: $typed, axis: .vertical)
            }
            Section("Result") {
                result(
                    "Keyboard reached app",
                    passed: server.foregroundPings + server.backgroundPings > 0
                )
                result("Reached app in background", passed: server.backgroundPings > 0)
                Text("Foreground pings: \(server.foregroundPings)")
                Text("Background pings: \(server.backgroundPings)")
            }
        }
        .onAppear {
            server.micReport = { [weak mic] in
                guard let mic else { return "mic=gone" }
                return "mic=\(mic.isRunning ? "on" : "off") buffers=\(mic.buffers)"
            }
            server.start()
        }
    }

    private func result(_ title: String, passed: Bool) -> some View {
        Text("\(passed ? "PASS" : "Waiting"): \(title)")
            .foregroundStyle(passed ? .green : .secondary)
            .bold(passed)
    }
}
