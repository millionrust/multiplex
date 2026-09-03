import SwiftUI
import UIKit

struct ControllerRootView: View {
    @ObservedObject var viewModel: ControllerViewModel
    @State private var showingPairing = false
    @State private var showingForgetConfirmation = false
    @State private var showingHostDetails = false
    @State private var pendingRoute: ControllerRemoteRouteKind?

    var body: some View {
        NavigationSplitView {
            List(selection: hostSelection) {
                Section("Paired Hosts") {
                    ForEach(viewModel.state.hosts) { host in
                        ControllerHostRow(
                            host: host,
                            selected: host.id == viewModel.state.selectedHostID
                        )
                        .tag(Optional(host.id))
                    }
                }
            }
            .navigationTitle("Fleet")
            .overlay {
                if viewModel.state.hosts.isEmpty {
                    ContentUnavailableView {
                        Label("No Paired Hosts", systemImage: "desktopcomputer")
                    } description: {
                        Text("Pair with TermiRust Desktop on the same private network.")
                    } actions: {
                        Button("Pair Host") { showingPairing = true }
                            .buttonStyle(.borderedProminent)
                    }
                }
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button { showingPairing = true } label: {
                        Label("Pair Host", systemImage: "plus")
                    }
                }
            }
        } detail: {
            ControllerSessionFleetView(
                state: viewModel.state,
                routes: viewModel.routeProjections,
                routeSelectionError: viewModel.routeSelectionError,
                onRetry: viewModel.retry,
                onForget: { showingForgetConfirmation = true },
                onShowDetails: { showingHostDetails = true },
                onOpenSession: viewModel.openReadOnlyTerminal,
                onSelectRoute: { pendingRoute = $0 }
            )
        }
        .navigationSplitViewStyle(.balanced)
        .sheet(isPresented: $showingPairing) {
            PairHostView(viewModel: viewModel, isPresented: $showingPairing)
        }
        .sheet(isPresented: $showingHostDetails) {
            if let host = viewModel.state.hosts.first(where: {
                $0.id == viewModel.state.selectedHostID
            }) {
                ControllerHostSettingsView(
                    host: host,
                    onReconnect: {
                        showingHostDetails = false
                        viewModel.retry()
                    },
                    onForget: {
                        showingHostDetails = false
                        showingForgetConfirmation = true
                    }
                )
            }
        }
        .confirmationDialog(
            "Forget this Host on this device?",
            isPresented: $showingForgetConfirmation,
            titleVisibility: .visible
        ) {
            Button("Forget on This Device", role: .destructive) {
                viewModel.forgetSelectedHost()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This removes the local pairing key and cache. It does not revoke the device on the Host.")
        }
        .confirmationDialog(
            "Switch connection route?",
            isPresented: Binding(
                get: { pendingRoute != nil },
                set: { if !$0 { pendingRoute = nil } }
            ),
            titleVisibility: .visible
        ) {
            if let pendingRoute {
                Button("Switch to \(ControllerPresentation.routeTitle(pendingRoute))") {
                    _ = viewModel.selectControllerRoute(
                        pendingRoute,
                        explicitlyConfirmed: true
                    )
                    self.pendingRoute = nil
                }
            }
            Button("Cancel", role: .cancel) { pendingRoute = nil }
        } message: {
            Text("The current Controller connection will close before the selected route starts.")
        }
        .fullScreenCover(isPresented: terminalPresented) {
            if let terminal = viewModel.activeTerminal {
                ControllerReadOnlyTerminalView(
                    viewModel: terminal,
                    onClose: viewModel.closeReadOnlyTerminal
                )
            }
        }
    }

    private var hostSelection: Binding<String?> {
        Binding(
            get: { viewModel.state.selectedHostID },
            set: { viewModel.selectHost(id: $0) }
        )
    }

    private var terminalPresented: Binding<Bool> {
        Binding(
            get: { viewModel.activeTerminal != nil },
            set: { if !$0 { viewModel.closeReadOnlyTerminal() } }
        )
    }
}

private struct ControllerHostRow: View {
    let host: HostSummary
    let selected: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.title3)
                .foregroundStyle(selected ? Color.accentColor : .secondary)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 3) {
                Text(ControllerPresentation.isolated(host.title))
                    .font(.body.weight(.semibold))
                    .lineLimit(2)
                Text(ControllerPresentation.isolated("\(host.route.address):\(host.route.port)"))
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.tertiary)
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

private struct ControllerSessionFleetView: View {
    let state: ControllerViewState
    let routes: [AppleControllerRouteProjection]
    let routeSelectionError: AppleControllerRouteCoordinatorError?
    let onRetry: () -> Void
    let onForget: () -> Void
    let onShowDetails: () -> Void
    let onOpenSession: (ControllerSessionSummary) -> Void
    let onSelectRoute: (ControllerRemoteRouteKind) -> Void

    var body: some View {
        Group {
            if state.selectedHostID == nil {
                ContentUnavailableView("Select a Host", systemImage: "rectangle.connected.to.line.below")
            } else {
                List {
                    Section {
                        ControllerStatusBanner(state: state, onRetry: onRetry)
                    }
                    if state.sessions.isEmpty {
                        Section {
                            ContentUnavailableView {
                                Label("No Open Terminals", systemImage: "terminal")
                            } description: {
                                Text(emptyMessage)
                            }
                            .frame(maxWidth: .infinity, minHeight: 220)
                            .listRowBackground(Color.clear)
                        }
                    } else {
                        let openTerminals = ControllerPresentation.openTerminals(state.sessions)
                        let previousSessions = ControllerPresentation.previousSessions(state.sessions)
                        if !openTerminals.isEmpty {
                            Section("Open Terminals") {
                                ForEach(openTerminals) { session in
                                    Button { onOpenSession(session) } label: {
                                        ControllerSessionRow(
                                            session: session,
                                            cached: state.isCachedReadOnly
                                        )
                                    }
                                    .buttonStyle(.plain)
                                    .disabled(
                                        state.isCachedReadOnly
                                            || state.connection != .readyReadOnly
                                    )
                                }
                            }
                        }
                        if !previousSessions.isEmpty {
                            Section("Previous Sessions") {
                                ForEach(previousSessions) { session in
                                    ControllerSessionRow(
                                        session: session,
                                        cached: state.isCachedReadOnly
                                    )
                                }
                            }
                        }
                    }
                    Section("Connection Route") {
                        ForEach(routes.filter { $0.selected || $0.available }) { route in
                            ControllerRouteRow(
                                route: route,
                                onSelect: { onSelectRoute(route.route) }
                            )
                        }
                        if routeSelectionError != nil {
                            Label("Route switch was not completed", systemImage: "exclamationmark.triangle")
                                .font(.caption)
                                .foregroundStyle(.orange)
                                .accessibilityAddTraits(.isStaticText)
                        }
                    }
                }
                .navigationTitle(selectedTitle)
                .toolbar {
                    ToolbarItemGroup(placement: .primaryAction) {
                        Button(action: onRetry) {
                            Label("Refresh", systemImage: "arrow.clockwise")
                        }
                        Button(action: onShowDetails) {
                            Label("Host Details", systemImage: "info.circle")
                        }
                        Button(role: .destructive, action: onForget) {
                            Label("Forget", systemImage: "trash")
                        }
                    }
                }
                .refreshable { onRetry() }
            }
        }
    }

    private var selectedTitle: String {
        state.hosts.first(where: { $0.id == state.selectedHostID })?.title ?? "Sessions"
    }

    private var emptyMessage: LocalizedStringKey {
        state.isCachedReadOnly
            ? "No terminals were saved in the last complete snapshot."
            : "Open a local or SSH terminal in TermiRust Desktop, then refresh."
    }
}

private struct ControllerRouteRow: View {
    let route: AppleControllerRouteProjection
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: ControllerPresentation.routeIcon(route.route))
                .foregroundStyle(route.selected ? Color.accentColor : .secondary)
                .frame(width: 24, height: 24)
            VStack(alignment: .leading, spacing: 3) {
                Text(ControllerPresentation.routeTitle(route.route))
                    .font(.body.weight(.semibold))
                Text(ControllerPresentation.routeStatus(route))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if route.selected {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(Color.accentColor)
                    .accessibilityLabel("Selected route")
            } else if route.available {
                Button("Use", action: onSelect)
                    .buttonStyle(.bordered)
            } else {
                Text("Not configured")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(minHeight: 48)
        .accessibilityElement(children: .contain)
    }
}

private struct ControllerStatusBanner: View {
    let state: ControllerViewState
    let onRetry: () -> Void

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 12) {
                statusContent
                Spacer(minLength: 8)
                retryButton
            }
            VStack(alignment: .leading, spacing: 10) {
                statusContent
                retryButton
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }

    private var statusContent: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon)
                .foregroundStyle(color)
                .font(.title3)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                detailText
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var retryButton: some View {
        if showsRetry {
            Button("Retry", action: onRetry)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .frame(minHeight: 44)
        }
    }

    private var showsRetry: Bool {
        if case .failed = state.connection { return true }
        return state.connection == .pairedOffline
    }

    private var title: LocalizedStringKey {
        if state.isCachedReadOnly { return "Cached · Read Only" }
        switch state.connection {
        case .readyReadOnly: return "Live · Read Only"
        case .connecting, .authenticating, .syncing: return "Connecting"
        case .failed: return "Host Unavailable"
        case .revoked: return "Pairing Revoked"
        case .incompatible: return "Update Required"
        default: return "Offline"
        }
    }

    @ViewBuilder
    private var detailText: some View {
        if let date = state.cacheUpdatedAt {
            Text("Last complete snapshot \(date.formatted(.relative(presentation: .named))).")
        } else {
            switch state.connection {
            case .readyReadOnly:
                Text("Open terminals are ready to view securely.")
            case .connecting, .authenticating, .syncing:
                Text("Authenticating directly with the selected Host.")
            case .failed(let failure):
                Text(failureMessage(failure))
            default:
                Text("Connect to the same LAN or VPN, then retry.")
            }
        }
    }

    private var icon: String {
        state.isCachedReadOnly ? "clock.arrow.circlepath" : (state.connection == .readyReadOnly ? "checkmark.shield" : "wifi.exclamationmark")
    }

    private var color: Color {
        state.isCachedReadOnly ? .orange : (state.connection == .readyReadOnly ? .green : .secondary)
    }

    private func failureMessage(_ failure: ControllerFailure) -> LocalizedStringKey {
        switch failure {
        case .authenticationFailed: return "Authentication failed. The device may have been revoked."
        case .sequenceGap: return "The Host changed during sync. Retry for a complete snapshot."
        case .resourceLimit: return "The Host has more data than this device can cache safely."
        case .keychainUnavailable: return "Unlock this device or pair again to restore the device key."
        case .malformedResponse: return "The Host returned an incompatible response."
        case .timedOut: return "The Host did not respond before the secure connection deadline."
        case .pairingUncertain: return "The Host may have saved this device, but confirmation was interrupted. Keep the pairing offer open and try again."
        default: return "Connect to the same LAN or VPN, then retry."
        }
    }
}

private struct ControllerSessionRow: View {
    let session: ControllerSessionSummary
    let cached: Bool
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        Group {
            if dynamicTypeSize.isAccessibilitySize {
                VStack(alignment: .leading, spacing: 8) {
                    sessionContent
                    freshnessBadge
                        .padding(.leading, 40)
                }
            } else {
                HStack(spacing: 12) {
                    sessionContent
                    Spacer(minLength: 8)
                    freshnessBadge
                    if canOpen {
                        Image(systemName: "chevron.right")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.tertiary)
                    }
                }
            }
        }
        .frame(minHeight: 52)
        .accessibilityElement(children: .combine)
    }

    private var sessionContent: some View {
        HStack(spacing: 12) {
            Image(systemName: lifecycleIcon)
                .foregroundStyle(lifecycleColor)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 4) {
                Text(ControllerPresentation.isolated(session.title))
                    .font(.body.weight(.medium))
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    Text(ControllerPresentation.originLabel(session.origin))
                    if let runtime = session.runtime {
                        Text(ControllerPresentation.isolated(runtime))
                            .fontDesign(.monospaced)
                    }
                    Text(
                        session.capabilities.contains(.sendInput)
                            ? LocalizedStringKey("Control available")
                            : LocalizedStringKey("View only")
                    )
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if session.project != nil || session.group != nil {
                    Text(metadata)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                AnyLayout(
                    dynamicTypeSize.isAccessibilitySize
                        ? AnyLayout(VStackLayout(alignment: .leading, spacing: 3))
                        : AnyLayout(HStackLayout(spacing: 8))
                ) {
                    Text(ControllerPresentation.lifecycleLabel(session.lifecycle))
                    if let activity = session.activity {
                        Text(ControllerPresentation.activityLabel(activity))
                    }
                    if session.hasWriter { Text("Writer active") }
                    if session.unreadCount > 0 {
                        Text(ControllerPresentation.unreadDescription(session.unreadCount))
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
    }

    private var freshnessBadge: some View {
        Text(freshnessText)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(freshnessColor)
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(freshnessColor.opacity(0.12))
            .clipShape(Capsule())
    }

    private var freshnessText: LocalizedStringKey {
        if cached { return "Cached" }
        return canOpen ? "Live" : "Closed"
    }

    private var freshnessColor: Color {
        if cached { return .orange }
        return canOpen ? .green : .secondary
    }

    private var canOpen: Bool {
        !cached && ControllerPresentation.isOpenTerminal(session)
    }

    private var metadata: String {
        [session.project, session.group]
            .compactMap { $0 }
            .map(ControllerPresentation.isolated)
            .joined(separator: " · ")
    }

    private var lifecycleIcon: String {
        switch session.lifecycle {
        case "live", "running", "running_app_attached": return "play.circle.fill"
        case "stopped", "exited": return "stop.circle"
        default: return "circle.dotted"
        }
    }

    private var lifecycleColor: Color {
        ["live", "running", "running_app_attached"].contains(session.lifecycle) ? .green : .secondary
    }
}

private struct PairHostView: View {
    @ObservedObject var viewModel: ControllerViewModel
    @Binding var isPresented: Bool
    @State private var showingScanner = false
    @State private var scannerFailure: ControllerScannerFailure?
    @State private var pairingOfferPasteError: String?

    var body: some View {
        NavigationStack {
            Form {
                if let challenge = viewModel.pairingChallenge {
                    Section("Compare on Both Devices") {
                        LabeledContent("Host") {
                            Text(ControllerPresentation.isolated(
                                "\(challenge.route.address):\(challenge.route.port)"
                            ))
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                        }
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Host Fingerprint")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(ControllerPresentation.isolated(challenge.hostFingerprint))
                                .font(.caption.monospaced())
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityLabel(
                                    "Host fingerprint \(ControllerPresentation.fingerprintForSpeech(challenge.hostFingerprint))"
                                )
                        }
                        Text(challenge.sas)
                            .font(.system(.largeTitle, design: .monospaced, weight: .bold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                            .textSelection(.enabled)
                            .accessibilityLabel("Security code \(challenge.sas.map(String.init).joined(separator: " "))")
                        Text("Only continue when this exact code is visible in TermiRust Desktop.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    Section {
                        if viewModel.state.connection == .pairing {
                            HStack(spacing: 12) {
                                ProgressView()
                                Text("Waiting for Host confirmation…")
                            }
                            .frame(maxWidth: .infinity, minHeight: 44)
                            .accessibilityElement(children: .combine)
                        } else {
                            Button("Codes Match") { viewModel.finishPairing(matches: true) }
                                .buttonStyle(.borderedProminent)
                                .frame(maxWidth: .infinity, minHeight: 44)
                            Button("Reject", role: .destructive) {
                                viewModel.finishPairing(matches: false)
                                isPresented = false
                            }
                            .frame(maxWidth: .infinity, minHeight: 44)
                        }
                    }
                } else {
                    if let pairingRecoveryMessage {
                        Section {
                            Label {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text("A New Pairing Offer Is Required")
                                        .font(.headline)
                                    Text(pairingRecoveryMessage)
                                        .font(.footnote)
                                        .foregroundStyle(.secondary)
                                }
                            } icon: {
                                Image(systemName: "arrow.clockwise.circle.fill")
                                    .foregroundStyle(.orange)
                            }
                            .accessibilityElement(children: .combine)
                        }
                    }
                    Section("Names") {
                        TextField("Host name", text: $viewModel.pairingHostName)
                            .textContentType(.name)
                        TextField("This device", text: $viewModel.pairingDeviceName)
                            .textContentType(.name)
                    }
                    Section("Pairing Offer") {
                        Button {
                            scannerFailure = nil
                            showingScanner = true
                        } label: {
                            Label("Scan QR Code", systemImage: "qrcode.viewfinder")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        VStack(alignment: .leading, spacing: 10) {
                            ZStack(alignment: .topLeading) {
                                if viewModel.pairingOfferText.isEmpty {
                                    Text("Paste pairing offer here")
                                        .font(.footnote)
                                        .foregroundStyle(.secondary)
                                        .padding(.horizontal, 12)
                                        .padding(.vertical, 14)
                                        .allowsHitTesting(false)
                                }
                                TextEditor(text: $viewModel.pairingOfferText)
                                    .font(.system(.footnote, design: .monospaced))
                                    .scrollContentBackground(.hidden)
                                    .padding(4)
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                    .accessibilityLabel("Pairing offer")
                            }
                            .frame(minHeight: 150)
                            .background(Color(uiColor: .secondarySystemBackground))
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .overlay {
                                RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .stroke(Color(uiColor: .separator), lineWidth: 1)
                            }
                            HStack(spacing: 10) {
                                Button {
                                    pastePairingOffer()
                                } label: {
                                    Label("Paste Offer", systemImage: "doc.on.clipboard")
                                        .frame(maxWidth: .infinity)
                                }
                                .buttonStyle(.borderedProminent)
                                Button {
                                    viewModel.pairingOfferText = ""
                                    pairingOfferPasteError = nil
                                } label: {
                                    Label("Clear", systemImage: "xmark.circle")
                                }
                                .buttonStyle(.bordered)
                                .disabled(viewModel.pairingOfferText.isEmpty)
                            }
                            if let pairingOfferPasteError {
                                Label(pairingOfferPasteError, systemImage: "exclamationmark.triangle")
                                    .font(.footnote)
                                    .foregroundStyle(.orange)
                                    .fixedSize(horizontal: false, vertical: true)
                            } else if !viewModel.pairingOfferText.isEmpty {
                                Label("Pairing offer ready", systemImage: "checkmark.circle.fill")
                                    .font(.footnote.weight(.semibold))
                                    .foregroundStyle(.green)
                            }
                        }
                        Text("In TermiRust Desktop, open Settings, Remote Devices, Add Controller, then copy the pairing offer here.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                        if let scannerFailure {
                            Text(scannerFailure == .permissionDenied
                                ? "Camera access is off. Paste the pairing offer instead, or enable Camera in Settings."
                                : "A camera is unavailable. Paste the pairing offer instead.")
                                .font(.footnote)
                                .foregroundStyle(.orange)
                        }
                    }
                    Section {
                        Button("Continue") { viewModel.beginPairing() }
                            .buttonStyle(.borderedProminent)
                            .frame(maxWidth: .infinity)
                            .disabled(
                                viewModel.pairingOfferText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                    || viewModel.pairingHostName.isEmpty
                                    || viewModel.pairingDeviceName.isEmpty
                            )
                    }
                }
            }
            .navigationTitle("Pair Host")
            .navigationBarTitleDisplayMode(.inline)
            .interactiveDismissDisabled(viewModel.pairingChallenge != nil)
            .onChange(of: viewModel.state.connection) { _, connection in
                if viewModel.pairingChallenge == nil,
                   viewModel.pairingOfferText.isEmpty,
                   connection != .pairing {
                    isPresented = false
                }
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        viewModel.cancelPairing()
                        isPresented = false
                    }
                }
            }
            .fullScreenCover(isPresented: $showingScanner) {
                ZStack(alignment: .topTrailing) {
                    ControllerQRCodeScanner { code in
                        viewModel.pairingOfferText = code
                        showingScanner = false
                    } onFailure: { failure in
                        scannerFailure = failure
                        showingScanner = false
                    }
                    Button {
                        showingScanner = false
                    } label: {
                        Image(systemName: "xmark")
                            .font(.headline)
                            .frame(width: 44, height: 44)
                            .background(.ultraThinMaterial, in: Circle())
                    }
                    .accessibilityLabel("Close Scanner")
                    .padding()
                }
                .ignoresSafeArea()
            }
        }
    }

    private func pastePairingOffer() {
        guard let offer = UIPasteboard.general.string?
            .trimmingCharacters(in: .whitespacesAndNewlines),
              !offer.isEmpty else {
            pairingOfferPasteError = "The clipboard does not contain a pairing offer."
            return
        }
        viewModel.pairingOfferText = offer
        pairingOfferPasteError = nil
    }

    private var pairingRecoveryMessage: String? {
        guard viewModel.pairingOfferText.isEmpty,
              case .failed(let failure) = viewModel.state.connection else {
            return nil
        }
        switch failure {
        case .offerExpired:
            return "The previous offer expired or was already used. Generate a fresh offer on the Host, then paste it below."
        case .timedOut, .networkUnavailable:
            return "The previous attempt could not finish. Check that both devices are on the same LAN or VPN, generate a fresh offer, and try again."
        case .pairingUncertain:
            return "Confirmation was interrupted. Check the Host's device list first; if this phone is absent, generate a fresh offer and pair again."
        default:
            return "The previous attempt did not complete and its one-use offer was discarded. Generate a fresh offer on the Host, then paste it below."
        }
    }
}

private struct ControllerHostSettingsView: View {
    let host: HostSummary
    let onReconnect: () -> Void
    let onForget: () -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Connection") {
                    LabeledContent("Host", value: ControllerPresentation.isolated(host.title))
                    LabeledContent("Route") {
                        Text(ControllerPresentation.isolated("\(host.route.address):\(host.route.port)"))
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                    }
                }
                Section("Host Identity") {
                    Text(ControllerPresentation.isolated(host.fingerprint))
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityLabel(
                            "Host fingerprint \(ControllerPresentation.fingerprintForSpeech(host.fingerprint))"
                        )
                }
                Section("Granted Capabilities") {
                    ForEach(Array(ControllerPresentation.capabilityLabels(bits: host.capabilityBits).enumerated()), id: \.offset) { _, label in
                        Label(label, systemImage: "checkmark.circle")
                    }
                    Text("Terminal monitoring is view-only. This app cannot send input unless interactive control is granted separately.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section {
                    Button(action: onReconnect) {
                        Label("Reconnect", systemImage: "arrow.clockwise")
                    }
                    Button(role: .destructive, action: onForget) {
                        Label("Forget on This Device", systemImage: "trash")
                    }
                } footer: {
                    Text("Forgetting removes the local key and cached summaries. It does not revoke this device on the Host.")
                }
            }
            .navigationTitle("Host Details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
