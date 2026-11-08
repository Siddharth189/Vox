import SwiftUI

@main
struct VoxSpikeApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

private enum SpikeResult {
    case notStarted
    case waiting(String)
    case passed(String)
    case failed(String)
}

struct ContentView: View {
    @State private var sent = ""
    @State private var result = SpikeResult.notStarted
    @State private var typed = ""

    private let poll = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        Form {
            Section("1. Signing") {
                Text(SpikeBox.diagnostics())
                    .font(.footnote.monospaced())
            }
            Section("2. Send a token") {
                Button("Send token to keyboard", action: send)
                if !sent.isEmpty {
                    Text(sent).font(.footnote.monospaced())
                }
            }
            Section("3. Switch to Vox Spike Keyboard, tap Run test") {
                TextField("Tap here, then use the globe key", text: $typed, axis: .vertical)
            }
            Section("Result") {
                resultView
            }
        }
        .onReceive(poll) { _ in check() }
    }

    @ViewBuilder
    private var resultView: some View {
        switch result {
        case .notStarted:
            Text("Not started")
        case .waiting(let message):
            Text(message)
        case .passed(let message):
            Text("PASS: \(message)").foregroundStyle(.green).bold()
        case .failed(let message):
            Text("FAIL: \(message)").foregroundStyle(.red).bold()
        }
    }

    private func send() {
        let token = "vox-\(UUID().uuidString.prefix(8))"
        if let problem = SpikeBox.write(token, to: SpikeBox.appFile) {
            result = .failed(problem)
            return
        }
        sent = token
        result = .waiting("Waiting for the keyboard")
    }

    private func check() {
        guard !sent.isEmpty, SpikeBox.read(SpikeBox.keyboardFile) == "ack:\(sent)" else {
            return
        }
        result = .passed("the keyboard received the token and replied")
    }
}
