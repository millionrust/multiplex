import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct TerminalSessionView: View {
    @ObservedObject var viewModel: HostListViewModel
    let host: MobileHost
    let framed: Bool
    let protectsTopSafeArea: Bool
    @State private var input = ""
    @State private var credential = ""
    @State private var terminalFontSize: CGFloat = 14
    @State private var pendingMultilinePaste: String?
    @State private var terminalGrid: TerminalGrid?
    @State private var lastSentTerminalGrid: TerminalGrid?
    @State private var showingPrivateKeyImporter = false
    @State private var controlModifierActive = false
    @State private var optionModifierActive = false

    init(
        viewModel: HostListViewModel,
        host: MobileHost,
        framed: Bool = true,
        protectsTopSafeArea: Bool = true
    ) {
        self.viewModel = viewModel
        self.host = host
        self.framed = framed
        self.protectsTopSafeArea = protectsTopSafeArea
    }

    var body: some View {
        GeometryReader { proxy in
            let compact = proxy.size.width < 700 || !framed
            let cornerRadius: CGFloat = framed && !compact ? 18 : 0
            let safeInsets = effectiveSafeAreaInsets(proxy.safeAreaInsets)
            let topSafeArea = !framed && protectsTopSafeArea ? safeInsets.top : 0
            let bottomSafeArea = safeInsets.bottom
            ZStack {
                Color.mobileBackground.ignoresSafeArea()
                if viewModel.privacyCovered {
                    directSSHPrivacyCover
                } else {
                    VStack(spacing: 0) {
                        sessionHeader(topSafeArea: topSafeArea)
                        if let failureMessage {
                            ConnectionWarningBanner(message: failureMessage)
                        }
                        HostKeyPinPanel(host: host, knownHost: viewModel.knownHost(for: host))
                        credentialPanel
                        terminalSurface
                        terminalControls
                        if pendingMultilinePaste == input, !input.isEmpty {
                            pasteWarning
                        }
                        inputRow(bottomSafeArea: bottomSafeArea)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color.mobilePanelBackground)
                    .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                    .overlay {
                        if cornerRadius > 0 {
                            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                                .stroke(Color.panelBorder)
                        }
                    }
                    .padding(framed && !compact ? 12 : 0)
                }
            }
        }
        .fileImporter(
            isPresented: $showingPrivateKeyImporter,
            allowedContentTypes: [.plainText, .data],
            allowsMultipleSelection: false
        ) { result in
            importPrivateKey(result)
        }
        .onChange(of: viewModel.privacyCovered) { _, covered in
            guard covered else { return }
            input = ""
            credential = ""
            pendingMultilinePaste = nil
            controlModifierActive = false
            optionModifierActive = false
        }
    }

    private func sessionHeader(topSafeArea: CGFloat) -> some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) {
                sessionTitle
                Spacer()
                sessionActions
            }
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    sessionTitle
                    Spacer()
                    StatusPill(statusText, color: statusColor)
                }
                HStack(spacing: 8) {
                    Button("Connect") {
                        viewModel.connectSelectedHost()
                    }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity)
                    Button("Disconnect") {
                        viewModel.disconnect()
                    }
                    .buttonStyle(.bordered)
                    .disabled(viewModel.connectionState == .disconnected)
                    .frame(maxWidth: .infinity)
                }
            }
        }
        .padding(14)
        .padding(.top, topSafeArea)
    }

    private var sessionTitle: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text(host.label)
                    .font(.title3.weight(.bold))
                    .lineLimit(1)
                StatusPill("Direct SSH", color: .blue)
            }
            Text("\(host.username)@\(host.host):\(host.port)")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    private var directSSHPrivacyCover: some View {
        VStack(spacing: 12) {
            Image(systemName: "eye.slash")
                .font(.title2)
            Text("Direct SSH terminal hidden while TermiRust is inactive")
                .font(.headline)
                .multilineTextAlignment(.center)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.mobilePanelBackground)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Direct SSH terminal content hidden for privacy")
    }

    private var sessionActions: some View {
        HStack(spacing: 8) {
            StatusPill(statusText, color: statusColor)
            Button("Connect") {
                viewModel.connectSelectedHost()
            }
            .buttonStyle(.borderedProminent)
            Button("Disconnect") {
                viewModel.disconnect()
            }
            .buttonStyle(.bordered)
            .disabled(viewModel.connectionState == .disconnected)
        }
    }

    private var credentialPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(credentialHelp)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            credentialInput
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    Button("Save Credential") {
                        viewModel.saveCredential(credential, for: host)
                        credential = ""
                    }
                    .buttonStyle(.bordered)
                    .disabled((host.auth.secretRef?.isEmpty ?? true) || credential.isEmpty)
                    Button("Remove") {
                        viewModel.deleteCredential(for: host)
                        credential = ""
                    }
                    .buttonStyle(.bordered)
                    .disabled(host.auth.secretRef?.isEmpty ?? true)
                    if host.auth.kind == .privateKey {
                        Button("Import Key File") {
                            showingPrivateKeyImporter = true
                        }
                        .buttonStyle(.bordered)
                        .disabled(host.auth.secretRef?.isEmpty ?? true)
                    }
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }

    private var failureMessage: String? {
        if case .failed(let message) = viewModel.connectionState {
            return message
        }
        return nil
    }

    @ViewBuilder
    private var credentialInput: some View {
        if host.auth.kind == .privateKey {
            TextField(credentialLabel, text: $credential, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .lineLimit(3...6)
                .font(.system(.caption, design: .monospaced))
                .disabled(host.auth.secretRef?.isEmpty ?? true)
        } else {
            SecureField(credentialLabel, text: $credential)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(host.auth.secretRef?.isEmpty ?? true)
        }
    }

    private var terminalSurface: some View {
        GeometryReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    if viewModel.terminalBuffer.lines.isEmpty {
                        Text("Terminal output will appear here.")
                            .foregroundStyle(Color.terminalMuted)
                    } else {
                        ForEach(Array(viewModel.terminalBuffer.lines.enumerated()), id: \.offset) { _, line in
                            Text(line.isEmpty ? " " : line)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .textSelection(.enabled)
                        }
                    }
                }
                .font(.system(size: terminalFontSize, design: .monospaced))
                .foregroundStyle(Color.terminalForeground)
                .padding(12)
            }
            .background(Color.terminalBackground)
            .clipShape(RoundedRectangle(cornerRadius: framed ? 14 : 0, style: .continuous))
            .onAppear {
                updateTerminalGrid(size: proxy.size)
            }
            .onChange(of: proxy.size) { _, newSize in
                updateTerminalGrid(size: newSize)
            }
            .onChange(of: terminalFontSize) { _, _ in
                updateTerminalGrid(size: proxy.size)
            }
            .onChange(of: viewModel.connectionState) { _, newState in
                if newState != .connected {
                    lastSentTerminalGrid = nil
                }
                updateTerminalGrid(size: proxy.size, forceResize: newState == .connected)
            }
        }
        .padding(.horizontal, framed ? 14 : 0)
        .padding(.vertical, framed ? 8 : 0)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .frame(minHeight: 220)
    }

    private var terminalControls: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Button("A-") {
                    terminalFontSize = max(10, terminalFontSize - 1)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                Button("A+") {
                    terminalFontSize = min(24, terminalFontSize + 1)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                Text("\(Int(terminalFontSize)) sp")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                if host.persistentSession.enabled {
                    StatusPill("tmux", color: .blue)
                }
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    Button("Ctrl") {
                        controlModifierActive.toggle()
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(controlModifierActive ? .blue : .secondary)
                    Button("Alt") {
                        optionModifierActive.toggle()
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(optionModifierActive ? .blue : .secondary)
                    ForEach(accessoryKeys, id: \.label) { key in
                        Button(key.label) {
                            viewModel.sendTerminalBytes(key.bytes)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                    }
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }

    private var pasteWarning: some View {
        HStack {
            Text("Multiline paste detected.")
                .font(.caption)
                .foregroundStyle(.orange)
            Spacer()
            Button("Confirm") {
                sendInput(force: true)
            }
            .buttonStyle(.borderedProminent)
            Button("Cancel") {
                pendingMultilinePaste = nil
            }
            .buttonStyle(.bordered)
        }
        .padding(10)
        .background(Color.orange.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(Color.orange.opacity(0.28))
        )
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }

    private func inputRow(bottomSafeArea: CGFloat) -> some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Command", text: $input, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .lineLimit(1...4)
                .onChange(of: input) { _, newValue in
                    if pendingMultilinePaste != newValue {
                        pendingMultilinePaste = nil
                    }
                }
            Button("Send") {
                sendInput()
            }
            .buttonStyle(.borderedProminent)
            .disabled(input.isEmpty || viewModel.connectionState != .connected)
        }
        .padding(14)
        .padding(.bottom, framed ? 0 : bottomSafeArea)
    }

    private func sendInput(force: Bool = false) {
        let modifierActive = controlModifierActive || optionModifierActive
        if !force && !modifierActive && input.contains("\n") {
            pendingMultilinePaste = input
            return
        }
        if modifierActive {
            viewModel.sendTerminalBytes(
                encodeTerminalInput(input, control: controlModifierActive, option: optionModifierActive)
            )
        } else {
            viewModel.sendTerminalInput(input)
        }
        input = ""
        controlModifierActive = false
        optionModifierActive = false
        pendingMultilinePaste = nil
    }

    private func importPrivateKey(_ result: Result<[URL], Error>) {
        do {
            guard case .success(let urls) = result, let url = urls.first else {
                return
            }
            let scoped = url.startAccessingSecurityScopedResource()
            defer {
                if scoped {
                    url.stopAccessingSecurityScopedResource()
                }
            }
            let key = try String(contentsOf: url, encoding: .utf8)
            viewModel.saveCredential(key, for: host)
            credential = ""
        } catch {
            viewModel.reportStatus(error.localizedDescription)
        }
    }

    private func effectiveSafeAreaInsets(_ proxyInsets: EdgeInsets) -> EdgeInsets {
        if proxyInsets.top > 0 || proxyInsets.bottom > 0 || proxyInsets.leading > 0 || proxyInsets.trailing > 0 {
            return proxyInsets
        }
        guard
            let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first,
            let window = scene.windows.first(where: { $0.isKeyWindow })
        else {
            return proxyInsets
        }
        let windowInsets = window.safeAreaInsets
        return EdgeInsets(
            top: windowInsets.top,
            leading: windowInsets.left,
            bottom: windowInsets.bottom,
            trailing: windowInsets.right
        )
    }

    private func updateTerminalGrid(size: CGSize, forceResize: Bool = false) {
        let grid = estimateTerminalGrid(size: size, fontSize: terminalFontSize)
        if terminalGrid != grid {
            terminalGrid = grid
        }
        guard viewModel.connectionState == .connected else {
            return
        }
        guard forceResize || lastSentTerminalGrid != grid else {
            return
        }
        lastSentTerminalGrid = grid
        viewModel.resizeTerminal(columns: grid.columns, rows: grid.rows)
    }

    private var accessoryKeys: [(label: String, bytes: Data)] {
        [
            ("Esc", Data([0x1b])),
            ("Tab", Data([0x09])),
            ("←", Data("\u{1b}[D".utf8)),
            ("↓", Data("\u{1b}[B".utf8)),
            ("↑", Data("\u{1b}[A".utf8)),
            ("→", Data("\u{1b}[C".utf8)),
            ("/", Data("/".utf8)),
            ("|", Data("|".utf8)),
            ("-", Data("-".utf8))
        ]
    }

    private var credentialLabel: String {
        switch host.auth.kind {
        case .password:
            return "SSH password"
        case .privateKey:
            return "Private key PEM"
        }
    }

    private var credentialHelp: String {
        if let secretRef = host.auth.secretRef, !secretRef.isEmpty {
            return "Credential: \(secretRef)"
        }
        return "No mobile secret reference exported for this host."
    }

    private var statusText: String {
        switch viewModel.connectionState {
        case .disconnected:
            return "Disconnected"
        case .connecting:
            return "Connecting"
        case .connected:
            return "Connected"
        case .failed:
            return "Failed"
        }
    }

    private var statusColor: Color {
        switch viewModel.connectionState {
        case .connected:
            return .green
        case .connecting:
            return .blue
        case .disconnected:
            return .secondary
        case .failed:
            return .red
        }
    }
}

struct StatusPill: View {
    let label: String
    let color: Color

    init(_ label: String, color: Color) {
        self.label = label
        self.color = color
    }

    var body: some View {
        Text(label)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .background(color.opacity(0.11))
            .clipShape(Capsule())
            .lineLimit(1)
    }
}

private struct ConnectionWarningBanner: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Connection blocked")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.red)
            Text(message)
                .font(.caption)
                .foregroundStyle(Color(red: 0.50, green: 0.11, blue: 0.11))
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.red.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(Color.red.opacity(0.25))
        )
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }
}

private struct HostKeyPinPanel: View {
    let host: MobileHost
    let knownHost: MobileKnownHost?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: knownHost == nil ? "exclamationmark.shield" : "checkmark.shield")
                .foregroundStyle(knownHost == nil ? .red : .green)
                .font(.body.weight(.semibold))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 4) {
                Text(knownHost == nil ? "Host key not pinned" : "Host key pinned")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(knownHost == nil ? .red : .green)
                Text(knownHost?.endpoint ?? host.knownHostEndpoint ?? "\(host.host):\(host.port)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(pinDetail)
                    .font(.caption2.monospaced())
                    .foregroundStyle(knownHost == nil ? Color(red: 0.50, green: 0.11, blue: 0.11) : .secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .background((knownHost == nil ? Color.red : Color.green).opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke((knownHost == nil ? Color.red : Color.green).opacity(0.24))
        )
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }

    private var pinDetail: String {
        guard let knownHost else {
            return "Export a known-host pin from desktop before connecting."
        }
        if let fingerprint = knownHost.fingerprint, !fingerprint.isEmpty {
            return fingerprint
        }
        return knownHost.publicKey.truncatedMiddle(maxLength: 52)
    }
}

private extension String {
    func truncatedMiddle(maxLength: Int) -> String {
        guard count > maxLength, maxLength > 8 else {
            return self
        }
        let prefixCount = maxLength / 2 - 2
        let suffixCount = maxLength - prefixCount - 3
        return "\(prefix(prefixCount))...\(suffix(suffixCount))"
    }
}

extension View {
    func mobilePanel() -> some View {
        self
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.mobilePanelBackground)
            .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .stroke(Color.panelBorder)
            )
    }
}

extension Color {
    static let mobileBackground = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.05, green: 0.06, blue: 0.08, alpha: 1)
            : UIColor(red: 0.96, green: 0.97, blue: 0.98, alpha: 1)
    })
    static let mobilePanelBackground = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.09, green: 0.10, blue: 0.12, alpha: 1)
            : UIColor.white
    })
    static let panelBorder = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.20, green: 0.23, blue: 0.28, alpha: 1)
            : UIColor(red: 0.88, green: 0.90, blue: 0.94, alpha: 1)
    })
    static let terminalBackground = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.02, green: 0.03, blue: 0.06, alpha: 1)
            : UIColor(red: 0.04, green: 0.06, blue: 0.13, alpha: 1)
    })
    static let terminalForeground = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.94, green: 0.95, blue: 0.97, alpha: 1)
            : UIColor(red: 0.90, green: 0.92, blue: 0.95, alpha: 1)
    })
    static let terminalMuted = Color(UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.62, green: 0.68, blue: 0.78, alpha: 1)
            : UIColor(red: 0.58, green: 0.64, blue: 0.72, alpha: 1)
    })
}
