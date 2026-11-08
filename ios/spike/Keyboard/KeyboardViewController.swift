import UIKit

/// Proves the two things the real keyboard depends on: exchanging data with
/// the app through the App Group, and typing into another app's text field.
final class KeyboardViewController: UIInputViewController {
    private let status = UILabel()

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
        let access = hasFullAccess ? "Full Access on" : "Full Access OFF"
        var outcome: String

        if let token = SpikeBox.read(SpikeBox.appFile) {
            if let problem = SpikeBox.write("ack:\(token)", to: SpikeBox.keyboardFile) {
                outcome = "Read the token, reply failed: \(problem)"
            } else {
                outcome = "Read \(token) and replied"
            }
            textDocumentProxy.insertText("Vox spike got \(token) ")
        } else {
            outcome = "No token found. Tap Send in the app first."
        }

        status.text = "\(access)\n\(SpikeBox.diagnostics())\n\(outcome)"
    }
}
