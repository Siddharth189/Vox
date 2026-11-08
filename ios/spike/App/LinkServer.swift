import Foundation
import Network

/// Answers the keyboard's commands on the loopback address.
final class LinkServer {
    var onState: (String) -> Void = { _ in }
    var onCommand: (String, @escaping (String) -> Void) -> Void = { _, reply in reply("err:not ready") }

    private var listener: NWListener?

    func start() {
        guard listener == nil, let port = NWEndpoint.Port(rawValue: SpikeLink.port) else {
            return
        }
        do {
            let parameters = NWParameters.tcp
            parameters.allowLocalEndpointReuse = true
            parameters.requiredLocalEndpoint = .hostPort(
                host: NWEndpoint.Host(SpikeLink.host),
                port: port
            )

            let listener = try NWListener(using: parameters)
            listener.stateUpdateHandler = { [weak self] state in
                self?.update(state)
            }
            listener.newConnectionHandler = { [weak self] connection in
                self?.handle(connection)
            }
            self.listener = listener
            listener.start(queue: .main)
        } catch {
            onState("Failed: \(error.localizedDescription)")
        }
    }

    private func update(_ state: NWListener.State) {
        switch state {
        case .ready:
            onState("Listening")
        case .failed(let error):
            onState("Failed: \(error.localizedDescription)")
            listener?.cancel()
            listener = nil
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                self?.start()
            }
        default:
            break
        }
    }

    private func handle(_ connection: NWConnection) {
        connection.start(queue: .main)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64) {
            [weak self] data, _, _, _ in
            guard let self, let data, let command = String(data: data, encoding: .utf8) else {
                connection.cancel()
                return
            }
            self.onCommand(command) { reply in
                DispatchQueue.main.async {
                    connection.send(
                        content: Data(reply.utf8),
                        contentContext: .defaultMessage,
                        isComplete: true,
                        completion: .contentProcessed { _ in connection.cancel() }
                    )
                }
            }
        }
    }
}
