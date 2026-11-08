import Foundation
import Security

/// Sends speech to Groq, whose translation mode returns English text for
/// Hindi, English, or mixed speech.
enum Transcriber {
    private static let endpoint = URL(string: "https://api.groq.com/openai/v1/audio/translations")!

    private struct Success: Decodable {
        let text: String
    }

    private struct Failure: Decodable {
        struct Detail: Decodable {
            let message: String
        }
        let error: Detail
    }

    static func translate(
        wav: Data,
        apiKey: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        let boundary = "vox-\(UUID().uuidString)"
        var request = URLRequest(url: endpoint, timeoutInterval: 30)
        request.httpMethod = "POST"
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue(
            "multipart/form-data; boundary=\(boundary)",
            forHTTPHeaderField: "Content-Type"
        )

        var body = Data()
        for (name, value) in [("model", "whisper-large-v3"), ("response_format", "json")] {
            body += Data("--\(boundary)\r\n".utf8)
            body += Data("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n\(value)\r\n".utf8)
        }
        body += Data("--\(boundary)\r\n".utf8)
        body += Data("Content-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n".utf8)
        body += Data("Content-Type: audio/wav\r\n\r\n".utf8)
        body += wav
        body += Data("\r\n--\(boundary)--\r\n".utf8)

        URLSession.shared.uploadTask(with: request, from: body) { data, response, error in
            if let error {
                completion(.failure(error))
                return
            }
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            let payload = data ?? Data()
            if status == 200, let success = try? JSONDecoder().decode(Success.self, from: payload) {
                completion(.success(success.text.trimmingCharacters(in: .whitespacesAndNewlines)))
                return
            }
            let detail = (try? JSONDecoder().decode(Failure.self, from: payload))?.error.message
            let message = "Groq error \(status): \(detail ?? "unexpected reply")"
            completion(.failure(NSError(
                domain: "Vox",
                code: status,
                userInfo: [NSLocalizedDescriptionKey: message]
            )))
        }.resume()
    }
}

/// Stores the API key in this app's Keychain, readable while in the background.
enum KeyStore {
    private static let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "vox.groq",
        kSecAttrAccount as String: "api-key",
    ]

    static func load() -> String? {
        var request = query
        request[kSecReturnData as String] = true
        var item: CFTypeRef?
        guard
            SecItemCopyMatching(request as CFDictionary, &item) == errSecSuccess,
            let data = item as? Data
        else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    static func save(_ key: String) -> Bool {
        SecItemDelete(query as CFDictionary)
        var item = query
        item[kSecValueData as String] = Data(key.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }
}
