import SwiftUI
import UniformTypeIdentifiers
import UIKit

struct EnrollmentDocument: FileDocument {
    static let readableContentTypes: [UTType] = [.json]
    var bytes: Data
    init(bytes: Data) { self.bytes = bytes }
    init(configuration: ReadConfiguration) throws {
        // A document representation is not an enrollment import or executable payload.
        guard let data = configuration.file.regularFileContents, (1...4096).contains(data.count) else {
            throw MobileReplicationError.Invalid
        }
        bytes = data
    }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        guard (1...4096).contains(bytes.count) else { throw MobileReplicationError.Invalid }
        return FileWrapper(regularFileWithContents: bytes)
    }
}

struct EnrollmentView: View {
    @StateObject private var model = EnrollmentViewModel()
    @Environment(\.dismiss) private var dismiss
    @State private var reviewed: Data?
    @State private var exporting = false
    @State private var document: EnrollmentDocument?
    @State private var notice: LocalizedStringKey?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(heading).font(.title2).accessibilityAddTraits(.isHeader)
                    Text("Local request only. Synchronization is unavailable here.")
                        .foregroundStyle(.secondary)
                    if model.loading { ProgressView().accessibilityLabel("Working") }
                    if let problem = model.problem { Text(problem).foregroundStyle(.red) }
                    if let request = model.snapshot.request {
                        Button {
                            UIPasteboard.general.setItems([[UTType.utf8PlainText.identifier: request]],
                                options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(120)])
                            notice = "Request copied."
                        } label: { Label("Copy request", systemImage: "doc.on.doc") }
                        .disabled(!ready)
                        ShareLink(item: String(decoding: request, as: UTF8.self)) {
                            Label("Share request", systemImage: "square.and.arrow.up")
                        }.disabled(!ready)
                        Button {
                            document = EnrollmentDocument(bytes: request)
                            exporting = true
                        } label: { Label("Export request", systemImage: "square.and.arrow.up.on.square") }
                        .disabled(!ready)
                        Button(role: .destructive) { reviewed = request } label: {
                            Text(model.snapshot.cancellationPending ? "Retry cancellation" : "Cancel request")
                        }.disabled(model.loading || exporting)
                    } else {
                        Button("Prepare request") { Task { await model.prepare() } }
                            .buttonStyle(.borderedProminent)
                            .disabled(model.loading || model.problem != nil)
                    }
                    if let notice { Text(notice).foregroundStyle(.secondary) }
                }
                .buttonStyle(.bordered)
                .frame(maxWidth: 640, alignment: .leading)
                .padding()
                .frame(maxWidth: .infinity)
            }
            .navigationTitle("Enrollment")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { if !exporting { dismiss() } }
                        .keyboardShortcut(.cancelAction)
                        .disabled(exporting)
                }
                ToolbarItem(placement: .primaryAction) {
                    Button { Task { await model.reload() } } label: {
                        Label("Reload request", systemImage: "arrow.clockwise")
                    }.disabled(model.loading || exporting)
                    .keyboardShortcut("r", modifiers: .command)
                }
            }
        }
        .task { await model.reload() }
        .onChange(of: model.snapshot.request) { _, _ in notice = nil }
        .alert("Cancel this request?", isPresented: Binding(get: { reviewed != nil }, set: { if !$0 { reviewed = nil } })) {
            Button("Keep request", role: .cancel) { reviewed = nil }
            Button("Confirm cancellation", role: .destructive) {
                guard let bytes = reviewed else { return }
                reviewed = nil
                Task { await model.cancel(bytes) }
            }
        } message: {
            Text("This removes this request and its enrollment identity. Other identities and terminal connections remain unchanged.")
        }
        .fileExporter(isPresented: $exporting, document: document, contentType: .json,
            defaultFilename: "termirust-enrollment") { result in
            switch result {
            case .success: notice = "Request exported."
            case .failure: notice = "Export failed. The saved request is unchanged."
            }
            document = nil
        }
    }

    private var ready: Bool { !model.loading && !exporting && model.problem == nil && !model.snapshot.cancellationPending }
    private var heading: LocalizedStringKey {
        if model.loading { return "Working" }
        if model.snapshot.cancellationPending { return "Cancellation pending" }
        if model.problem != nil { return "Request status unavailable" }
        return model.snapshot.request == nil ? "No enrollment request" : "Request pending"
    }
}
