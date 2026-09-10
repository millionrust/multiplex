import SwiftUI

@main
struct DonorApp: App {
    private let ready: Bool
    init() {
        do {
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            try Data("{\"fixture\":\"C06\"}".utf8).write(to: directory.appendingPathComponent("c06-transfer.json"), options: .atomic)
            ready = true
        } catch { ready = false }
    }
    var body: some Scene {
        WindowGroup { Text(ready ? "Fixture ready" : "Fixture failed") }
    }
}
