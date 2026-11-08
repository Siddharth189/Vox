import SwiftUI

@main
struct VoxSpikeApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

final class VoxEngine: ObservableObject {
    @Published private(set) var mic = "Off"
    @Published private(set) var link = "Not started"
    @Published private(set) var last = "Nothing yet"
    @Published private(set) var hasKey = KeyStore.load() != nil

    private let recorder = Recorder()
    private let server = LinkServer()

    init() {
        recorder.onStatus = { [weak self] in self?.mic = $0 }
        server.onState = { [weak self] in self?.link = $0 }
        server.onCommand = { [weak self] command, reply in
            guard let self else {
                reply("err:app unavailable")
                return
            }
            self.handle(command, reply: reply)
        }
    }

    /// Opening the app starts a session once a key is saved.
    func activate() {
        server.start()
        if hasKey {
            recorder.start()
        }
    }

    func endSession() {
        recorder.stop(reason: "Off")
    }

    func saveKey(_ key: String) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, KeyStore.save(trimmed) else {
            last = "Could not save the key"
            return
        }
        hasKey = true
        activate()
    }

    private func handle(_ command: String, reply: @escaping (String) -> Void) {
        switch command {
        case SpikeLink.start:
            guard recorder.isRunning else {
                reply("err:Open the Vox Spike app to start a session")
                return
            }
            recorder.beginCapture()
            reply("ok")
        case SpikeLink.cancel:
            recorder.cancelCapture()
            reply("ok")
        case SpikeLink.stop:
            guard let key = KeyStore.load() else {
                recorder.cancelCapture()
                reply("err:Add your Groq key in the app")
                return
            }
            recorder.endCapture { [weak self] wav in
                Transcriber.translate(wav: wav, apiKey: key) { result in
                    DispatchQueue.main.async {
                        self?.finish(result, reply: reply)
                    }
                }
            }
        default:
            reply("err:Unknown command")
        }
    }

    private func finish(_ result: Result<String, Error>, reply: (String) -> Void) {
        switch result {
        case .success(let text) where text.isEmpty:
            last = "Empty result"
            reply("empty")
        case .success(let text):
            last = text
            reply("text:\(text)")
        case .failure(let error):
            last = error.localizedDescription
            reply("err:\(error.localizedDescription)")
        }
    }
}

struct ContentView: View {
    @StateObject private var engine = VoxEngine()
    @Environment(\.scenePhase) private var scenePhase
    @State private var key = ""

    var body: some View {
        Form {
            Section("Groq API key") {
                SecureField(engine.hasKey ? "Saved. Paste to replace" : "Paste your key", text: $key)
                Button("Save key") {
                    engine.saveKey(key)
                    key = ""
                }
                .disabled(key.isEmpty)
            }
            Section("Session") {
                Text("Mic: \(engine.mic)")
                Text("Link: \(engine.link)")
                Button("End session", role: .destructive) { engine.endSession() }
            }
            .font(.footnote.monospaced())
            Section("Last dictation") {
                Text(engine.last)
                    .textSelection(.enabled)
            }
        }
        .onChange(of: scenePhase, initial: true) { _, phase in
            if phase == .active {
                engine.activate()
            }
        }
    }
}
