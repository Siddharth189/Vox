import Network
import UIKit

/// Proves the two things the real keyboard depends on: talking to the app
/// over loopback, and typing into another app's text field.
final class KeyboardViewController: UIInputViewController {
    private let status = UILabel()
    private var connection: NWConnection?
    private var attempt = 0

    override func viewDidLoad() {
        super.viewDidLoad()

        status.numberOfLines = 0
        status.font = .preferredFont(forTextStyle: .footnote)
        status.text = "Ready"

        let run = UIButton(configuration: .borderedProminent())
        run.setTitle("Run test", for: .normal)
        run.addTarget(self, action: #selector(runTest), for: .touchUpInside)

        let next = UIButton(configuration: .bordered())
        next.setImage(UIImage(systemName: "globe"), for: .normal)
        next.isHidden = !needsInputModeSwitchKey
        next.addTarget(
            self,
            action: #selector(handleInputModeList(from:with:)),
            for: .allTouchEvents
        )

        let buttons = UIStackView(arrangedSubviews: [next, run])
        buttons.spacing = 8

        let stack = UIStackView(arrangedSubviews: [status, buttons])
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        let height = view.heightAnchor.constraint(equalToConstant: 220)
        height.priority = .defaultHigh
        NSLayoutConstraint.activate([
            height,
            stack.topAnchor.constraint(equalTo: view.topAnchor, constant: 12),
            stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
        ])
    }

    @objc private func runTest() {
        guard hasFullAccess else {
            status.text = "FAIL: Allow Full Access is off"
            return
        }
        guard let port = NWEndpoint.Port(rawValue: SpikeLink.port) else { return }

        connection?.cancel()
        attempt += 1
        let current = attempt
        status.text = "Connecting…"

        let link = NWConnection(host: NWEndpoint.Host(SpikeLink.host), port: port, using: .tcp)
        connection = link

        link.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                self?.exchange(over: link, attempt: current)
            case .failed(let error), .waiting(let error):
                self?.finish(attempt: current, message: "FAIL: \(error.localizedDescription)")
            default:
                break
            }
        }
        link.start(queue: .main)

        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in
            self?.finish(attempt: current, message: "FAIL: the app did not answer in 4s")
        }
    }

    private func exchange(over link: NWConnection, attempt current: Int) {
        link.send(content: Data("ping".utf8), completion: .contentProcessed { _ in })
        link.receive(minimumIncompleteLength: 1, maximumLength: 128) {
            [weak self] data, _, _, error in
            guard let data, let reply = String(data: data, encoding: .utf8) else {
                let reason = error?.localizedDescription ?? "empty reply"
                self?.finish(attempt: current, message: "FAIL: \(reason)")
                return
            }
            self?.succeed(attempt: current, reply: reply)
        }
    }

    private func succeed(attempt current: Int, reply: String) {
        guard current == attempt, connection != nil else { return }
        textDocumentProxy.insertText("Vox link OK: \(reply)\n")
        finish(attempt: current, message: "PASS: \(reply)")
    }

    /// Only the first outcome of an attempt counts; later ones are ignored.
    private func finish(attempt current: Int, message: String) {
        guard current == attempt, connection != nil else { return }
        status.text = message
        connection?.cancel()
        connection = nil
    }
}
