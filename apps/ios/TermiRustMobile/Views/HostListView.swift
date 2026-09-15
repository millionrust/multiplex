import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct HostListView: View {
    @ObservedObject var viewModel: HostListViewModel
    @State private var showingImporter = false
    @State private var vaultPassphrase = ""

    var body: some View {
        ZStack {
            Color.mobileBackground.ignoresSafeArea()
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    productHeader
                    importPanel
                    statusPanel
                    hostSection
                }
                .padding(16)
            }
        }
        .navigationTitle("TermiRust")
        .navigationBarTitleDisplayMode(.inline)
        .fileImporter(
            isPresented: $showingImporter,
            allowedContentTypes: [.json],
            allowsMultipleSelection: false
        ) { result in
            defer {
                vaultPassphrase = ""
            }
            guard case .success(let urls) = result, let url = urls.first else {
                return
            }
            viewModel.importEncryptedVault(from: url, passphrase: vaultPassphrase)
        }
    }

    private var productHeader: some View {
        HStack(spacing: 10) {
            ZStack {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color.terminalBackground)
                    .frame(width: 36, height: 36)
                Text(">")
                    .font(.headline.weight(.bold))
                    .foregroundStyle(.white)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text("TermiRust")
                    .font(.title2.weight(.bold))
                Text("Mobile terminal")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var importPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Vault")
                .font(.headline)
            SecureField("Vault passphrase", text: $vaultPassphrase)
                .textContentType(.password)
                .textFieldStyle(.roundedBorder)
            Button {
                showingImporter = true
            } label: {
                Label("Import Encrypted Vault", systemImage: "square.and.arrow.down")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(vaultPassphrase.isEmpty)
            Button {
                copyPairingRequest()
            } label: {
                Label("Copy Pairing Request", systemImage: "doc.on.doc")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            if viewModel.hasStoredEncryptedVault {
                HStack(spacing: 8) {
                    Button("Unlock Saved Vault") {
                        viewModel.unlockStoredEncryptedVault(passphrase: vaultPassphrase)
                        vaultPassphrase = ""
                    }
                    .buttonStyle(.bordered)
                    .disabled(vaultPassphrase.isEmpty)
                    Button("Forget") {
                        viewModel.forgetStoredEncryptedVault()
                    }
                    .buttonStyle(.bordered)
                }
            }
        }
        .mobilePanel()
    }

    private func copyPairingRequest() {
        do {
            UIPasteboard.general.string = try viewModel.pairingRequestText()
            viewModel.reportStatus("Pairing request copied. Import it on desktop to authorize this device.")
        } catch {
            viewModel.reportStatus(error.localizedDescription)
        }
    }

    @ViewBuilder
    private var statusPanel: some View {
        if let error = viewModel.importError {
            Text(error)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .mobilePanel()
        }
    }

    private var hostSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Connections")
                .font(.headline)
            if viewModel.hosts.isEmpty {
                Text("No Connections imported yet.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .mobilePanel()
            } else {
                ForEach(viewModel.hosts) { host in
                    HostCard(
                        host: host,
                        selected: viewModel.selectedHost?.id == host.id
                    ) {
                        viewModel.selectedHost = host
                    }
                }
            }
        }
    }
}

private struct HostCard: View {
    let host: MobileHost
    let selected: Bool
    let onSelect: () -> Void

    var body: some View {
        Button(action: onSelect) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(host.label)
                        .font(.headline)
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    Spacer()
                    if host.persistentSession.enabled {
                        StatusPill("tmux", color: .blue)
                    }
                }
                Text("\(host.username)@\(host.host):\(host.port)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if let sessionName = host.persistentSession.sessionName {
                    Text(sessionName)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.blue)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(selected ? Color.blue.opacity(0.08) : Color(.secondarySystemBackground))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(selected ? Color.blue : Color.panelBorder)
            )
        }
        .buttonStyle(.plain)
    }
}
