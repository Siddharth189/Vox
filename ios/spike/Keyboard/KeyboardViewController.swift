import Network
import UIKit

/// Hold to talk. The app does the recording and the speech request; this
/// keyboard only signals it over loopback and types the result.
final class KeyboardViewController: UIInputViewController {
    private enum Phase {
        case idle, starting, recording, transcribing
    }

    private static let minimumHold: TimeInterval = 0.35

    private let status = UILabel()
    private let talk = UIButton(configuration: .borderedProminent())
    private var phase = Phase.idle
    private var pressedAt = Date()
    private var releasedEarly = false

    override func viewDidLoad() {
        super.viewDidLoad()

        status.numberOfLines = 2
        status.textAlignment = .center
        status.font = .preferredFont(forTextStyle: .footnote)
        show("Hold the button and speak")

        talk.setTitle("Hold to talk", for: .normal)
        talk.addTarget(self, action: #selector(pressBegan), for: .touchDown)
        for event: UIControl.Event in [.touchUpInside, .touchUpOutside, .touchCancel] {
            talk.addTarget(self, action: #selector(pressEnded), for: event)
        }

        let next = key(symbol: "globe")
        next.isHidden = !needsInputModeSwitchKey
        next.addTarget(
            self,
            action: #selector(handleInputModeList(from:with:)),
            for: .allTouchEvents
        )

        let space = key(title: "space", action: #selector(insertSpace))
        let delete = key(symbol: "delete.left", action: #selector(deleteBack))
        let enter = key(symbol: "return", action: #selector(insertReturn))
        space.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let keys = UIStackView(arrangedSubviews: [next, space, delete, enter])
        keys.spacing = 6

        let stack = UIStackView(arrangedSubviews: [status, talk, keys])
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        let height = view.heightAnchor.constraint(equalToConstant: 230)
        height.priority = .defaultHigh
        NSLayoutConstraint.activate([
            height,
            stack.topAnchor.constraint(equalTo: view.topAnchor, constant: 10),
            stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -10),
            stack.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -10),
            keys.heightAnchor.constraint(equalToConstant: 44),
        ])
    }

    private func key(title: String? = nil, symbol: String? = nil, action: Selector? = nil) -> UIButton {
        let button = UIButton(configuration: .bordered())
        button.setTitle(title, for: .normal)
        if let symbol {
            button.setImage(UIImage(systemName: symbol), for: .normal)
        }
        if let action {
            button.addTarget(self, action: action, for: .touchUpInside)
        }
        return button
    }

    private func show(_ message: String) {
        status.text = message
    }

    @objc private func insertSpace() { textDocumentProxy.insertText(" ") }
    @objc private func deleteBack() { textDocumentProxy.deleteBackward() }
    @objc private func insertReturn() { textDocumentProxy.insertText("\n") }

    @objc private func pressBegan() {
        guard phase == .idle else { return }
        guard hasFullAccess else {
            show("Turn on Allow Full Access for this keyboard")
            return
        }
        phase = .starting
        releasedEarly = false
        pressedAt = Date()
        show("Connecting…")
        send(SpikeLink.start, timeout: 3) { [weak self] reply in
            self?.started(reply)
        }
    }

    @objc private func pressEnded() {
        switch phase {
        case .starting:
            releasedEarly = true
        case .recording:
            finishRecording()
        case .idle, .transcribing:
            break
        }
    }

    private func started(_ reply: String?) {
        guard phase == .starting else { return }
        guard reply == "ok" else {
            phase = .idle
            show(message(from: reply))
            return
        }
        phase = .recording
        show("Listening…")
        if releasedEarly {
            finishRecording()
        }
    }

    private func finishRecording() {
        guard Date().timeIntervalSince(pressedAt) >= Self.minimumHold else {
            phase = .idle
            show("Hold a little longer")
            send(SpikeLink.cancel, timeout: 2) { _ in }
            return
        }
        phase = .transcribing
        show("Writing…")
        send(SpikeLink.stop, timeout: 35) { [weak self] reply in
            self?.finished(reply)
        }
    }

    private func finished(_ reply: String?) {
        phase = .idle
        if let reply, reply.hasPrefix("text:") {
            textDocumentProxy.insertText(String(reply.dropFirst(5)) + " ")
            show("Hold the button and speak")
        } else if reply == "empty" {
            show("Didn't catch that, try again")
        } else {
            show(message(from: reply))
        }
    }

    private func message(from reply: String?) -> String {
        guard let reply else { return "Open the Vox Spike app to start a session" }
        return reply.hasPrefix("err:") ? String(reply.dropFirst(4)) : reply
    }

    /// One command per connection. All callbacks run on the main queue.
    private func send(_ command: String, timeout: TimeInterval, done: @escaping (String?) -> Void) {
        guard let port = NWEndpoint.Port(rawValue: SpikeLink.port) else {
            done(nil)
            return
        }
        let link = NWConnection(host: NWEndpoint.Host(SpikeLink.host), port: port, using: .tcp)
        var isDone = false
        let finish: (String?) -> Void = { reply in
            guard !isDone else { return }
            isDone = true
            link.cancel()
            done(reply)
        }

        link.stateUpdateHandler = { state in
            switch state {
            case .ready:
                link.send(content: Data(command.utf8), completion: .contentProcessed { _ in })
                Self.readReply(from: link, soFar: Data(), finish: finish)
            case .failed, .waiting:
                finish(nil)
            default:
                break
            }
        }
        link.start(queue: .main)
        DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { finish(nil) }
    }

    private static func readReply(
        from link: NWConnection,
        soFar: Data,
        finish: @escaping (String?) -> Void
    ) {
        link.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, isComplete, error in
            var all = soFar
            if let data {
                all.append(data)
            }
            if isComplete || error != nil {
                finish(all.isEmpty ? nil : String(data: all, encoding: .utf8))
            } else {
                readReply(from: link, soFar: all, finish: finish)
            }
        }
    }
}
