import SwiftUI
import UniformTypeIdentifiers

@main
struct ReaderApp: App {
    var body: some Scene { WindowGroup { ReaderView() } }
}

struct ReaderView: View {
    @State private var selecting = false
    @State private var status = "Idle"
    var body: some View {
        VStack(spacing: 16) {
            Text(status).accessibilityIdentifier("outcome")
            Button("Select fixture") { selecting = true }
        }
        .sheet(isPresented: $selecting) {
            FixturePicker { selected in
                selecting = false
                guard let selected else { status = "Cancelled"; return }
                Task {
                    // Probe and release separately: the real reader must acquire its own access.
                    let scoped = selected.startAccessingSecurityScopedResource()
                    let home = URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath().path
                    let external = !selected.resolvingSymlinksInPath().path.hasPrefix(home + "/")
                    if scoped { selected.stopAccessingSecurityScopedResource() }
                    guard scoped else { status = "No security scope"; return }
                    guard external else {
                        status = "Not external"; return
                    }
                    do {
                        let bytes = try await ReplicationDocumentReader().read(selected, kind: .enrollmentBundle)
                        status = bytes == Data("{\"fixture\":\"C06\"}".utf8) ? "Verified external scoped read" : "Byte mismatch"
                    } catch { status = "Read failed" }
                }
            }
        }
    }
}

private struct FixturePicker: UIViewControllerRepresentable {
    let selected: (URL?) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(selected: selected) }
    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.json], asCopy: false)
        picker.delegate = context.coordinator
        return picker
    }
    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}
    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let selected: (URL?) -> Void
        init(selected: @escaping (URL?) -> Void) { self.selected = selected }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { selected(urls.first) }
        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { selected(nil) }
    }
}
