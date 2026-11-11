import Foundation
import Security

/// Sends speech to Groq's Whisper.
enum Transcriber {
    private static let base = "https://api.groq.com/openai/v1/audio/"

    /// Biases Whisper toward writing Hindi and English the way they were spoken,
    /// each in its own script, instead of translating either one.
    private static let hinglishHint =
        "आज मैं office जाऊँगा, meeting है, and then I will call you. Please वो file भेज देना।"

    private struct Success: Decodable {
        let text: String
    }

    /// English text for Hindi, English, or mixed speech.
    static func translate(
        wav: Data,
        apiKey: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        upload(wav: wav, path: "translations", fields: [], apiKey: apiKey, completion: completion)
    }

    /// The words as spoken, without translation. Hindi mostly comes back in
    /// Devanagari and English in Latin letters, so it still needs a rewrite.
    static func transcribeMixed(
        wav: Data,
        apiKey: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        let fields = [("language", "hi"), ("prompt", hinglishHint)]
        upload(wav: wav, path: "transcriptions", fields: fields, apiKey: apiKey) { result in
            // On silence Whisper can echo its hint back. That means nothing was said.
            if case .success(let text) = result, !text.isEmpty, hinglishHint.contains(text) {
                completion(.success(""))
            } else {
                completion(result)
            }
        }
    }

    private static func upload(
        wav: Data,
        path: String,
        fields: [(String, String)],
        apiKey: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        let boundary = "vox-\(UUID().uuidString)"
        var request = URLRequest(url: URL(string: base + path)!, timeoutInterval: 30)
        request.httpMethod = "POST"
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue(
            "multipart/form-data; boundary=\(boundary)",
            forHTTPHeaderField: "Content-Type"
        )

        var body = Data()
        let all = [("model", "whisper-large-v3"), ("response_format", "json")] + fields
        for (name, value) in all {
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
            completion(.failure(GroqError.make(status: status, payload: payload)))
        }.resume()
    }
}

/// Turns a Groq error reply into a readable error.
enum GroqError {
    private struct Failure: Decodable {
        struct Detail: Decodable {
            let message: String
        }
        let error: Detail
    }

    static func make(status: Int, payload: Data) -> NSError {
        let detail = (try? JSONDecoder().decode(Failure.self, from: payload))?.error.message
        return NSError(
            domain: "Vox",
            code: status,
            userInfo: [NSLocalizedDescriptionKey: "Groq error \(status): \(detail ?? "unexpected reply")"]
        )
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
