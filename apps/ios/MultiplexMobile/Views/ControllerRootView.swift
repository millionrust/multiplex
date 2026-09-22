import SwiftUI
import UIKit

struct ControllerRootView: View {
    @ObservedObject var viewModel: ControllerViewModel
    @State private var showingPairing = false
    @State private var showingEnrollment = false
    @State private var showingForgetConfirmation = false
    @State private var showingHostDetails = false
    @State private var showingSSHConfiguration = false
    @State private var showingRelayConfiguration = false
    @State private var pendingRoute: ControllerRemoteRouteKind?

    var body: some View {
        NavigationSplitView {
            List(selection: hostSelection) {
                Section("Paired Hosts") {
                    ForEach(viewModel.state.hosts) { host in
                        ControllerHostRow(
                            host: host,
                            selected: host.id == viewModel.state.selectedHostID,
                            picture: viewModel.screens.lastPictures[host.id]
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
                        Text("Pair with Multiplex Desktop on the same private network.")
                    } actions: {
                        Button("Pair a Computer") { showingPairing = true }
                            .buttonStyle(.borderedProminent)
                    }
                }
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button("Enrollment") { showingEnrollment = true }
                    } label: {
                        Label("Device actions", systemImage: "ellipsis.circle")
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button { showingPairing = true } label: {
                        Label("Pair a Computer", systemImage: "plus")
                    }
                }
            }
        } detail: {
            ControllerSessionFleetView(
                state: viewModel.state,
                routeAdvice: viewModel.routeAdvice,
                routes: viewModel.routeProjections,
                routeSelectionError: viewModel.routeSelectionError,
                onRetry: viewModel.retry,
                onForget: { showingForgetConfirmation = true },
                onShowDetails: { showingHostDetails = true },
                onOpenSession: viewModel.openReadOnlyTerminal,
                onSelectRoute: { pendingRoute = $0 },
                onConfigureSSH: { showingSSHConfiguration = true },
                onConfigureRelay: { showingRelayConfiguration = true },
                screens: viewModel.screens,
                canWatch: viewModel.canWatchSelectedHost,
                onStartPreview: viewModel.startScreenPreview,
                onStopPreview: viewModel.stopScreenPreview,
                onOpenScreen: viewModel.openScreen
            )
        }
        .navigationSplitViewStyle(.balanced)
        .sheet(isPresented: $showingEnrollment) { EnrollmentView() }
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
        .sheet(isPresented: $showingSSHConfiguration) {
            SSHControllerConfigurationView(
                configuration: viewModel.selectedSSHConfiguration,
                suggestedEndpoint: viewModel.selectedHost?.route.address ?? "",
                configurationError: viewModel.routeConfigurationError,
                onSave: { endpoint, port, username, pin, authentication, secret in
                    viewModel.configureSSHRoute(
                        endpoint: endpoint,
                        port: port,
                        username: username,
                        hostKeyPin: pin,
                        authentication: authentication,
                        secret: secret
                    )
                },
                onRemove: {
                    viewModel.removeSSHRoute()
                    showingSSHConfiguration = false
                }
            )
        }
        .sheet(isPresented: $showingRelayConfiguration) {
            RelayControllerConfigurationView(
                configuration: viewModel.selectedRelayConfiguration,
                configurationError: viewModel.routeConfigurationError,
                onSave: { endpoint, pin, routeID, epoch, credential in
                    viewModel.configureRelayRoute(
                        endpoint: endpoint,
                        spkiPin: pin,
                        routeID: routeID,
                        revocationEpoch: epoch,
                        admissionCredential: credential
                    )
                },
                onRemove: {
                    viewModel.removeRelayRoute()
                    showingRelayConfiguration = false
                }
            )
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
        .fullScreenCover(isPresented: screenPresented) {
            if let screen = viewModel.screens.viewer {
                ControllerScreenViewerSheet(
                    model: screen,
                    screens: viewModel.screens,
                    title: viewModel.selectedHost?.displayName ?? "Screen",
                    routeName: viewModel.selectedRoute.map(ControllerPresentation.routeTitle),
                    onClose: viewModel.closeScreen
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

    private var screenPresented: Binding<Bool> {
        Binding(
            get: { viewModel.screens.viewer != nil },
            set: { if !$0 { viewModel.closeScreen() } }
        )
    }
}

/// The full screen, with a way back to the computer's page.
private struct ControllerScreenViewerSheet: View {
    @ObservedObject var model: RemoteScreenViewModel
    @ObservedObject var screens: ControllerScreenCoordinator
    let title: String
    let routeName: String?
    let onClose: () -> Void

    var body: some View {
        NavigationStack {
            RemoteScreenView(
                model: model,
                onRequestControl: model.requestControl,
                onReleaseControl: model.releaseControl,
                reconnecting: screens.reconnecting,
                routeName: routeName
            )
            .navigationTitle(ControllerPresentation.isolated(title))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done", action: onClose)
                }
            }
        }
    }
}

private struct ControllerHostRow: View {
    let host: HostSummary
    let selected: Bool
    /// The last picture this phone saw of the computer, when it has watched it.
    var picture: CGImage?

    var body: some View {
        HStack(spacing: 12) {
            if let picture {
                Image(decorative: picture, scale: 1)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 44, height: 28)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                    .overlay(
                        RoundedRectangle(cornerRadius: 4)
                            .strokeBorder(Color.secondary.opacity(0.3))
                    )
                    .accessibilityLabel("Last picture of this computer")
            } else {
                Image(systemName: "desktopcomputer")
                    .font(.title3)
                    .foregroundStyle(selected ? Color.accentColor : .secondary)
                    .frame(width: 28, height: 28)
            }
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

/// The computer's screen on its own page: a picture about once a second, and the way in.
private struct ControllerScreenPreviewCard: View {
    let preview: RemoteScreenViewModel?
    let lastPicture: CGImage?
    let unavailable: ControllerScreenUnavailable?
    let onOpenScreen: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let preview {
                // The live preview redraws itself; a plain `let` would not be observed, so the
                // picture and the line naming its display are rendered by an observing view.
                ControllerScreenPreviewPicture(model: preview, lastPicture: lastPicture)
            } else {
                ControllerScreenPlaceholder(
                    lastPicture: lastPicture,
                    waiting: unavailable == nil
                )
                if let caption {
                    Text(caption)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Button(action: onOpenScreen) {
                Label("Open Screen", systemImage: "display")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(unavailable != nil)
        }
        .padding(.vertical, 4)
    }

    private var caption: String? {
        switch unavailable {
        case .notGranted:
            return "This computer has not given this phone screen access."
        case let .failed(reason):
            return reason
        case nil:
            return "Waiting for the first picture."
        }
    }
}

/// Redraws as the preview session applies each picture.
private struct ControllerScreenPreviewPicture: View {
    @ObservedObject var model: RemoteScreenViewModel
    let lastPicture: CGImage?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let image = model.image ?? lastPicture {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .accessibilityLabel("Preview of this computer's screen")
            } else {
                ControllerScreenPlaceholder(lastPicture: nil, waiting: true)
            }
            Text(caption)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var caption: String {
        if case let .closed(reason) = model.state {
            return reason
        }
        guard let name = model.displayName else { return "About one picture a second." }
        return "\(ControllerPresentation.isolated(name)) · about one picture a second"
    }
}

private struct ControllerScreenPlaceholder: View {
    let lastPicture: CGImage?
    let waiting: Bool

    var body: some View {
        if let lastPicture {
            Image(decorative: lastPicture, scale: 1)
                .resizable()
                .aspectRatio(contentMode: .fit)
                .frame(maxWidth: .infinity)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .opacity(0.6)
                .accessibilityLabel("The last picture of this computer's screen")
        } else {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color.secondary.opacity(0.12))
                .aspectRatio(16.0 / 10.0, contentMode: .fit)
                .overlay {
                    if waiting {
                        ProgressView()
                    } else {
                        Image(systemName: "display.trianglebadge.exclamationmark")
                            .font(.title)
                            .foregroundStyle(.secondary)
                    }
                }
                .accessibilityHidden(true)
        }
    }
}

private struct ControllerSessionFleetView: View {
    let state: ControllerViewState
    let routeAdvice: ControllerRouteAdvice?
    let routes: [AppleControllerRouteProjection]
    let routeSelectionError: AppleControllerRouteCoordinatorError?
    let onRetry: () -> Void
    let onForget: () -> Void
    let onShowDetails: () -> Void
    let onOpenSession: (ControllerSessionSummary) -> Void
    let onSelectRoute: (ControllerRemoteRouteKind) -> Void
    let onConfigureSSH: () -> Void
    let onConfigureRelay: () -> Void
    @ObservedObject var screens: ControllerScreenCoordinator
    let canWatch: Bool
    let onStartPreview: () -> Void
    let onStopPreview: () -> Void
    let onOpenScreen: () -> Void

    var body: some View {
        Group {
            if state.selectedHostID == nil {
                ContentUnavailableView("Select a Host", systemImage: "rectangle.connected.to.line.below")
            } else {
                List {
                    Section {
                        ControllerStatusBanner(state: state, advice: routeAdvice, onRetry: onRetry)
                    }
                    if canWatch {
                        Section("This Computer's Screen") {
                            ControllerScreenPreviewCard(
                                preview: screens.preview,
                                lastPicture: state.selectedHostID
                                    .flatMap { screens.lastPictures[$0] },
                                unavailable: screens.unavailable,
                                onOpenScreen: onOpenScreen
                            )
                        }
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
                        ForEach(routes) { route in
                            ControllerRouteRow(
                                route: route,
                                onSelect: { onSelectRoute(route.route) },
                                onConfigureSSH: onConfigureSSH,
                                onConfigureRelay: onConfigureRelay
                            )
                        }
                        if routeSelectionError != nil {
                            Label("Route switch was not completed", systemImage: "exclamationmark.triangle")
                                .font(.caption)
                                .foregroundStyle(Color.slateAttention)
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
                // The connection carries one session at a time, so the preview runs only while
                // this page is on screen, and only once the fleet has finished loading.
                .onAppear { if canWatch { onStartPreview() } }
                .onDisappear(perform: onStopPreview)
                .onChange(of: state.connection) { _, _ in
                    if canWatch { onStartPreview() }
                }
                .onChange(of: state.selectedHostID) { _, _ in
                    onStopPreview()
                }
            }
        }
    }

    private var selectedTitle: String {
        state.hosts.first(where: { $0.id == state.selectedHostID })?.title ?? "Sessions"
    }

    private var emptyMessage: LocalizedStringKey {
        state.isCachedReadOnly
            ? "No terminals were saved in the last complete snapshot."
            : "Open a local or SSH terminal in Multiplex Desktop, then refresh."
    }
}

private struct ControllerRouteRow: View {
    let route: AppleControllerRouteProjection
    let onSelect: () -> Void
    let onConfigureSSH: () -> Void
    let onConfigureRelay: () -> Void

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
            VStack(alignment: .trailing, spacing: 6) {
                if route.selected {
                    Label("Selected", systemImage: "checkmark.circle.fill")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Color.accentColor)
                } else if route.available {
                    Button("Use", action: onSelect)
                        .buttonStyle(.bordered)
                } else if route.route != .ssh && route.route != .selfHostedRelay {
                    Text("Not configured")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                }
                if route.route == .ssh {
                    Button(route.available ? "Edit" : "Configure", action: onConfigureSSH)
                        .buttonStyle(.bordered)
                }
                if route.route == .selfHostedRelay {
                    Button(route.available ? "Edit" : "Configure", action: onConfigureRelay)
                        .buttonStyle(.bordered)
                }
            }
        }
        .frame(minHeight: 48)
        .accessibilityElement(children: .contain)
    }
}

private struct RelayControllerConfigurationView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var endpoint: String
    @State private var spkiPin: String
    @State private var routeID: String
    @State private var epoch: String
    @State private var credential = ""
    @State private var localError: String?
    @State private var showingRemoveConfirmation = false

    let hasExistingConfiguration: Bool
    let configurationError: String?
    let onSave: (String, String, String, UInt64, String) -> Bool
    let onRemove: () -> Void

    init(
        configuration: ControllerRemoteRouteConfiguration?,
        configurationError: String?,
        onSave: @escaping (String, String, String, UInt64, String) -> Bool,
        onRemove: @escaping () -> Void
    ) {
        _endpoint = State(initialValue: configuration?.endpoint ?? "")
        _spkiPin = State(initialValue: configuration?.trustPin ?? "")
        _routeID = State(initialValue: configuration?.relayRouteID ?? "")
        _epoch = State(initialValue: configuration?.relayRevocationEpoch.map(String.init) ?? "0")
        self.hasExistingConfiguration = configuration != nil
        self.configurationError = configurationError
        self.onSave = onSave
        self.onRemove = onRemove
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Paste the connection package created by your relay operator. The relay carries only encrypted Controller frames, and the admission credential stays in Keychain.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                    Button {
                        importPackageFromClipboard()
                    } label: {
                        Label("Paste Controller Package", systemImage: "doc.on.clipboard")
                    }
                }
                Section("Relay") {
                    TextField("WSS endpoint", text: $endpoint)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    TextField("SPKI pin (sha256/...)", text: $spkiPin, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .fontDesign(.monospaced)
                    TextField("Route ID (Base64)", text: $routeID, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .fontDesign(.monospaced)
                    TextField("Revocation epoch", text: $epoch)
                        .keyboardType(.numberPad)
                    SecureField("Admission credential (Base64)", text: $credential)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    Text("Saving an edit requires entering the admission credential again. Multiplex never switches routes automatically.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let message = localError ?? configurationError {
                    Section {
                        Label(message, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(Color.slateError)
                    }
                }
                if hasExistingConfiguration {
                    Section {
                        Button("Remove Self-hosted Relay Route", role: .destructive) {
                            showingRemoveConfirmation = true
                        }
                    }
                }
            }
            .navigationTitle("Self-hosted Relay")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(!canSave)
                }
            }
            .confirmationDialog(
                "Remove this relay route?",
                isPresented: $showingRemoveConfirmation,
                titleVisibility: .visible
            ) {
                Button("Remove Route", role: .destructive, action: onRemove)
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This deletes the relay configuration and admission credential from this device. Paired Hosts, cached Sessions, and local Session history are kept.")
            }
        }
    }

    private var parsedEpoch: UInt64? { UInt64(epoch) }

    private var canSave: Bool {
        endpoint.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("wss://")
            && spkiPin.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("sha256/")
            && !routeID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && parsedEpoch != nil
            && !credential.isEmpty
    }

    private func save() {
        guard let parsedEpoch else { return }
        if onSave(endpoint, spkiPin, routeID, parsedEpoch, credential) {
            dismiss()
        } else {
            localError = "Check every field and make sure this device is unlocked."
        }
    }

    private func importPackageFromClipboard() {
        guard let text = UIPasteboard.general.string else {
            localError = "Copy the controller-route.json contents, then try again."
            return
        }
        do {
            let package = try ControllerRelayRoutePackage.decode(text)
            endpoint = package.endpoint
            spkiPin = package.spkiPin
            routeID = package.routeID
            epoch = String(package.revocationEpoch)
            credential = package.admissionCredential
            localError = nil
        } catch {
            localError = "The clipboard does not contain a valid Multiplex controller relay package."
        }
    }
}

private struct SSHControllerConfigurationView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var endpoint: String
    @State private var port: String
    @State private var username: String
    @State private var hostKeyPin: String
    @State private var authentication: ControllerSSHAuthenticationKind
    @State private var secret = ""
    @State private var localError: String?
    @State private var showingRemoveConfirmation = false

    let hasExistingConfiguration: Bool
    let configurationError: String?
    let onSave: (
        String,
        UInt16,
        String,
        String,
        ControllerSSHAuthenticationKind,
        String
    ) -> Bool
    let onRemove: () -> Void

    init(
        configuration: ControllerRemoteRouteConfiguration?,
        suggestedEndpoint: String,
        configurationError: String?,
        onSave: @escaping (
            String,
            UInt16,
            String,
            String,
            ControllerSSHAuthenticationKind,
            String
        ) -> Bool,
        onRemove: @escaping () -> Void
    ) {
        _endpoint = State(initialValue: configuration?.endpoint ?? suggestedEndpoint)
        _port = State(initialValue: configuration?.port.map(String.init) ?? "22")
        _username = State(initialValue: configuration?.username ?? "")
        _hostKeyPin = State(initialValue: configuration?.trustPin ?? "")
        _authentication = State(initialValue: configuration?.sshAuthentication ?? .privateKey)
        self.hasExistingConfiguration = configuration != nil
        self.configurationError = configurationError
        self.onSave = onSave
        self.onRemove = onRemove
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Use SSH only to carry the encrypted Multiplex Controller protocol. Pair on your private network first; routes never switch automatically.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Section("SSH Server") {
                    TextField("Host or IP address", text: $endpoint)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Port", text: $port)
                        .keyboardType(.numberPad)
                    TextField("Username", text: $username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Pinned host key or SHA256 fingerprint", text: $hostKeyPin, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .lineLimit(2...4)
                }
                Section("Authentication") {
                    Picker("Method", selection: $authentication) {
                        Text("Private Key").tag(ControllerSSHAuthenticationKind.privateKey)
                        Text("Password").tag(ControllerSSHAuthenticationKind.password)
                    }
                    .pickerStyle(.segmented)
                    SecureField(
                        authentication == .password ? "SSH password" : "Paste OpenSSH private key",
                        text: $secret
                    )
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    Text("The credential is stored in Keychain on this device. Saving an edit requires entering it again.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let message = localError ?? configurationError {
                    Section {
                        Label(message, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(Color.slateError)
                    }
                }
                if hasExistingConfiguration {
                    Section {
                        Button("Remove SSH Controller Route", role: .destructive) {
                            showingRemoveConfirmation = true
                        }
                    }
                }
            }
            .navigationTitle("Controller over SSH")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(!canSave)
                }
            }
            .confirmationDialog(
                "Remove this SSH Controller route?",
                isPresented: $showingRemoveConfirmation,
                titleVisibility: .visible
            ) {
                Button("Remove Route", role: .destructive, action: onRemove)
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Its credential and configuration will be deleted from this device. Multiplex will not switch to another route automatically.")
            }
        }
    }

    private var parsedPort: UInt16? { UInt16(port) }

    private var canSave: Bool {
        !endpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && parsedPort != nil
            && !username.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !hostKeyPin.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !secret.isEmpty
    }

    private func save() {
        guard let parsedPort else { return }
        if onSave(endpoint, parsedPort, username, hostKeyPin, authentication, secret) {
            dismiss()
        } else {
            localError = "Check every field and make sure this device is unlocked."
        }
    }
}

private struct ControllerStatusBanner: View {
    let state: ControllerViewState
    var advice: ControllerRouteAdvice?
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
                if let adviceMessage {
                    Text(adviceMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// Said only while it still fits: the Tailscale note once connected, the rest after a failure.
    private var adviceMessage: LocalizedStringKey? {
        switch (advice, state.connection) {
        case (.localNetworkBlockedWhileTailscaleWorks?, .readyReadOnly):
            return "Connected over Tailscale because this network could not reach the computer directly. If Tailscale uses an exit node, turn on “Allow local network access” in Tailscale."
        case (.remoteAccessOff?, .failed):
            return "The computer turned the connection away. Check that Remote access is on in Multiplex on the computer."
        case (.notOnComputersNetwork?, .failed):
            return "This phone is not on the computer's network, and no Tailscale, SSH, or relay route is set up."
        case (.needsRemoteRoute?, .failed):
            return "Away from the computer's network. Set up Tailscale, SSH, or a relay to connect from here."
        default:
            return nil
        }
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
    @State private var target: ControllerPairingTarget?
    @State private var showingOtherWays = false

    var body: some View {
        NavigationStack {
            PairComputerChooserView(
                viewModel: viewModel,
                browser: viewModel.computerBrowser,
                onChoose: { target = $0 },
                onOtherWays: { showingOtherWays = true }
            )
            .navigationTitle("Pair a Computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        viewModel.cancelPairing()
                        isPresented = false
                    }
                }
            }
            .navigationDestination(item: $target) { target in
                PairCodeView(viewModel: viewModel, target: target)
            }
            .navigationDestination(isPresented: $showingOtherWays) {
                PairOfferView(viewModel: viewModel, isPresented: $isPresented)
            }
        }
        .interactiveDismissDisabled(
            viewModel.pairingChallenge != nil || viewModel.state.connection == .pairing
        )
        .onAppear { viewModel.computerBrowser.start() }
        .onDisappear { viewModel.computerBrowser.stop() }
        .onChange(of: viewModel.pairingCompletion) { _, completion in
            if completion != nil { isPresented = false }
        }
    }
}

private struct PairComputerChooserView: View {
    @ObservedObject var viewModel: ControllerViewModel
    @ObservedObject var browser: ControllerComputerBrowser
    let onChoose: (ControllerPairingTarget) -> Void
    let onOtherWays: () -> Void
    @State private var showingAddressError = false

    var body: some View {
        Form {
            Section {
                if browser.computers.isEmpty {
                    if browser.isUnavailable {
                        Label(
                            "Allow Local Network access for Multiplex in Settings to find computers.",
                            systemImage: "wifi.exclamationmark"
                        )
                        .font(.footnote)
                        .foregroundStyle(Color.slateAttention)
                    } else {
                        HStack(spacing: 12) {
                            ProgressView()
                            Text("Looking for computers…")
                                .foregroundStyle(.secondary)
                        }
                        .frame(minHeight: 44)
                        .accessibilityElement(children: .combine)
                    }
                } else {
                    ForEach(browser.computers) { computer in
                        Button { onChoose(.discovered(computer)) } label: {
                            HStack(spacing: 12) {
                                Image(systemName: "desktopcomputer")
                                    .foregroundStyle(Color.accentColor)
                                    .frame(width: 28, height: 28)
                                Text(ControllerPresentation.isolated(computer.name))
                                    .font(.body.weight(.semibold))
                                    .foregroundStyle(.primary)
                                Spacer(minLength: 8)
                                Image(systemName: "chevron.right")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.tertiary)
                            }
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityElement(children: .combine)
                    }
                }
            } header: {
                Text("Computers on This Network")
            } footer: {
                Text("In Multiplex on your computer, choose Pair phone to show a six-digit code.")
            }
            Section {
                TextField("mac.tailnet.ts.net:55123", text: $viewModel.pairingAddressText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .keyboardType(.URL)
                    .fontDesign(.monospaced)
                    .accessibilityLabel("Computer address")
                    .onSubmit(chooseAddress)
                Button("Continue", action: chooseAddress)
                    .disabled(
                        viewModel.pairingAddressText
                            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                if showingAddressError {
                    Label(
                        "Enter the address as host:port, for example 100.101.102.103:55123.",
                        systemImage: "exclamationmark.triangle"
                    )
                    .font(.footnote)
                    .foregroundStyle(Color.slateAttention)
                    .fixedSize(horizontal: false, vertical: true)
                }
            } header: {
                Text("Enter Address")
            } footer: {
                Text("Use a Tailscale name or private address when the computer is on another network.")
            }
            Section {
                Button(action: onOtherWays) {
                    Label("Other Ways to Pair", systemImage: "qrcode")
                }
            }
        }
        .onChange(of: viewModel.pairingAddressText) { _, _ in
            showingAddressError = false
        }
    }

    private func chooseAddress() {
        let text = viewModel.pairingAddressText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (try? ControllerTypedAddress(text)) != nil else {
            showingAddressError = true
            return
        }
        showingAddressError = false
        onChoose(.address(text))
    }
}

private struct PairCodeView: View {
    @ObservedObject var viewModel: ControllerViewModel
    let target: ControllerPairingTarget
    @State private var attempted = false
    @FocusState private var codeFocused: Bool

    var body: some View {
        Form {
            Section {
                LabeledContent("Computer") {
                    Text(ControllerPresentation.isolated(target.displayName))
                        .font(.body.weight(.semibold))
                        .lineLimit(2)
                }
            }
            if isPairing {
                Section {
                    HStack(spacing: 12) {
                        ProgressView()
                        Text("Pairing…")
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .accessibilityElement(children: .combine)
                }
            } else if attempted, let failureMessage {
                Section {
                    Label(failureMessage, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(Color.slateError)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityElement(children: .combine)
                }
            }
            Section {
                TextField("000000", text: codeBinding)
                    .keyboardType(.numberPad)
                    .textContentType(.oneTimeCode)
                    .font(.system(.largeTitle, design: .monospaced, weight: .bold))
                    .multilineTextAlignment(.center)
                    .focused($codeFocused)
                    .disabled(isPairing)
                    .accessibilityLabel("Pairing code")
            } header: {
                Text("Pairing Code")
            } footer: {
                Text("Enter the six-digit code shown on the computer.")
            }
            Section("Device Name") {
                TextField("This device", text: $viewModel.pairingDeviceName)
                    .textContentType(.name)
                    .disabled(isPairing)
            }
            Section {
                if isPairing {
                    Button("Cancel", role: .cancel) { viewModel.cancelPairing() }
                        .frame(maxWidth: .infinity, minHeight: 44)
                } else {
                    Button("Pair") {
                        attempted = true
                        codeFocused = false
                        viewModel.pairWithCode(target: target)
                    }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity)
                    .disabled(!viewModel.isPairingCodeComplete || viewModel.pairingDeviceName.isEmpty)
                }
            }
        }
        .navigationTitle("Enter Code")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(isPairing)
        .onAppear { codeFocused = true }
    }

    private var codeBinding: Binding<String> {
        Binding(
            get: { viewModel.pairingCode },
            set: { viewModel.updatePairingCode($0) }
        )
    }

    private var isPairing: Bool {
        viewModel.state.connection == .pairing
    }

    private var failureMessage: LocalizedStringKey? {
        guard case .failed(let failure) = viewModel.state.connection else { return nil }
        switch failure {
        case .cancelled:
            return nil
        case .codeRejected:
            return "That code didn't work. Check the code on the computer and try again."
        case .invalidAddress:
            return "That address can't be used. Enter a private or Tailscale address as host:port."
        case .offerExpired:
            return "The code expired. Show a new code on the computer and try again."
        case .timedOut, .networkUnavailable:
            return "Couldn't reach the computer. Check that both devices are on the same network or Tailscale, then try again."
        case .keychainUnavailable:
            return "Unlock this device, then try again."
        case .pairingUncertain:
            return "The computer may have saved this device, but confirmation was interrupted. Check its device list before pairing again."
        default:
            return "Pairing didn't finish. Try again."
        }
    }
}

private struct PairOfferView: View {
    @ObservedObject var viewModel: ControllerViewModel
    @Binding var isPresented: Bool
    @State private var showingScanner = false
    @State private var scannerFailure: ControllerScannerFailure?
    @State private var pairingOfferPasteError: String?

    var body: some View {
        Group {
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
                        Text("Only continue when this exact code is visible in Multiplex Desktop.")
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
                                    .foregroundStyle(Color.slateAttention)
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
                                    .foregroundStyle(Color.slateAttention)
                                    .fixedSize(horizontal: false, vertical: true)
                            } else if !viewModel.pairingOfferText.isEmpty {
                                Label("Pairing offer ready", systemImage: "checkmark.circle.fill")
                                    .font(.footnote.weight(.semibold))
                                    .foregroundStyle(Color.slateDone)
                            }
                        }
                        Text("In Multiplex Desktop, open Settings, Remote Devices, Add Controller, then copy the pairing offer here.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                        if let scannerFailure {
                            Text(scannerFailure == .permissionDenied
                                ? "Camera access is off. Paste the pairing offer instead, or enable Camera in Settings."
                                : "A camera is unavailable. Paste the pairing offer instead.")
                                .font(.footnote)
                                .foregroundStyle(Color.slateAttention)
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
            .navigationTitle("Other Ways to Pair")
            .navigationBarTitleDisplayMode(.inline)
            .navigationBarBackButtonHidden(isBusy)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    if isBusy {
                        Button("Cancel") {
                            viewModel.cancelPairing()
                            isPresented = false
                        }
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

    private var isBusy: Bool {
        viewModel.pairingChallenge != nil || viewModel.state.connection == .pairing
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
