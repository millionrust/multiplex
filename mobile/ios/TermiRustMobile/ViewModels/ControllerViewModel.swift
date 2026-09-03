import Foundation
import SwiftUI
import UIKit

@MainActor
final class ControllerViewModel: ObservableObject {
    @Published private(set) var state = ControllerViewState.empty
    @Published private(set) var pairingChallenge: ControllerPairingChallenge?
    @Published var pairingOfferText = ""
    @Published var pairingHostName = "My Mac"
    @Published var pairingDeviceName = UIDevice.current.name
    @Published private(set) var activeTerminal: ControllerTerminalViewModel?
    @Published private(set) var routeProjections: [AppleControllerRouteProjection]
    @Published private(set) var routeSelectionError: AppleControllerRouteCoordinatorError?

    private let routeConnections: AppleControllerRouteConnections
    private let hostStore: PairedHostStore?
    private let cacheStore: ControllerFleetCacheStore?
    private let retryPolicy: ControllerRetryPolicy
    private let defaults: UserDefaults
    private var routeCoordinator: AppleControllerRouteCoordinator
    private var hostRecords: [PairedHostRecord] = []
    private var cache = ControllerFleetCache()
    private let deviceID: UUID
    private var operation: Task<Void, Never>?

    init(
        connectionActor: (any ControllerConnecting)? = nil,
        routeConnections: AppleControllerRouteConnections? = nil,
        hostStore: PairedHostStore? = nil,
        cacheStore: ControllerFleetCacheStore? = nil,
        defaults: UserDefaults = .standard,
        retryPolicy: ControllerRetryPolicy = .live
    ) {
        self.deviceID = Self.loadDeviceID(defaults: defaults)
        self.retryPolicy = retryPolicy
        self.defaults = defaults
        if let routeConnections {
            self.routeConnections = routeConnections
        } else if let connectionActor {
            self.routeConnections = AppleControllerRouteConnections(
                privateNetwork: connectionActor
            )
        } else {
            let blobStore = ControllerKeychainBlobStore()
            self.routeConnections = AppleControllerRouteConnections(
                privateNetwork: try? ControllerConnectionActor(blobStore: blobStore)
            )
        }
        self.hostStore = hostStore ?? (try? PairedHostStore())
        self.cacheStore = cacheStore ?? (try? ControllerFleetCacheStore())
        var coordinator = AppleControllerRouteCoordinator(
            availability: self.routeConnections.availability
        )
        if let rawRoute = defaults.string(forKey: Self.selectedRouteDefaultsKey),
           let savedRoute = ControllerRemoteRouteKind(rawValue: rawRoute),
           savedRoute != .localIPC {
            _ = try? coordinator.restorePersistedSelection(savedRoute)
        } else if self.routeConnections.privateNetwork != nil {
            _ = try? coordinator.select(.privateNetwork, explicitlyConfirmed: true)
        }
        self.routeCoordinator = coordinator
        self.routeProjections = coordinator.projections
        self.routeSelectionError = nil
        guard self.routeConnections.privateNetwork != nil,
              self.hostStore != nil,
              self.cacheStore != nil else {
            state = ControllerViewState(
                hosts: [],
                selectedHostID: nil,
                sessions: [],
                connection: .failed(.storageUnavailable),
                cacheUpdatedAt: nil,
                isCachedReadOnly: false
            )
            return
        }
        operation = Task { [weak self] in await self?.restore() }
    }

    deinit {
        operation?.cancel()
    }

    var selectedHost: PairedHostRecord? {
        hostRecords.first { $0.id == state.selectedHostID }
    }

    var selectedRoute: ControllerRemoteRouteKind? {
        routeCoordinator.selected
    }

    private var selectedConnection: (any ControllerConnecting)? {
        guard let selectedRoute else { return nil }
        return routeConnections.connection(for: selectedRoute)
    }

    func beginPairing() {
        guard let connectionActor = routeConnections.privateNetwork else { return }
        operation?.cancel()
        pairingChallenge = nil
        state = replacing(connection: .pairing, sessions: state.sessions)
        let offer = pairingOfferText
        let hostName = pairingHostName
        let deviceName = pairingDeviceName
        operation = Task { [weak self] in
            guard let self else { return }
            do {
                let challenge = try await connectionActor.beginPairing(
                    offerText: offer,
                    hostName: hostName,
                    deviceName: deviceName,
                    deviceID: deviceID
                )
                guard !Task.isCancelled else { return }
                pairingChallenge = challenge
                state = replacing(connection: .sasReady(challenge.sas), sessions: state.sessions)
            } catch {
                guard !Task.isCancelled else { return }
                pairingOfferText = ""
                state = replacing(connection: .failed(Self.failure(error)), sessions: state.sessions)
            }
        }
    }

    func finishPairing(matches: Bool) {
        guard let connectionActor = routeConnections.privateNetwork, let hostStore else { return }
        operation?.cancel()
        state = replacing(connection: .pairing, sessions: state.sessions)
        operation = Task { [weak self] in
            guard let self else { return }
            do {
                let record = try await connectionActor.finishPairing(matches: matches)
                hostRecords = try await hostStore.upsert(record)
                defaults.set(record.id, forKey: Self.selectedHostDefaultsKey)
                pairingChallenge = nil
                pairingOfferText = ""
                state = makeState(
                    selectedHostID: record.id,
                    sessions: [],
                    connection: .pairedOffline,
                    cacheUpdatedAt: nil,
                    isCached: false
                )
                await refresh(host: record)
            } catch {
                guard !Task.isCancelled else { return }
                pairingChallenge = nil
                pairingOfferText = ""
                state = replacing(connection: .failed(Self.failure(error)), sessions: state.sessions)
            }
        }
    }

    func cancelPairing() {
        operation?.cancel()
        operation = nil
        pairingChallenge = nil
        Task { await routeConnections.privateNetwork?.cancel() }
        state = makeState(
            selectedHostID: state.selectedHostID,
            sessions: state.sessions,
            connection: hostRecords.isEmpty ? .unpaired : .pairedOffline,
            cacheUpdatedAt: state.cacheUpdatedAt,
            isCached: state.isCachedReadOnly
        )
    }

    func selectHost(id: String?) {
        guard state.selectedHostID != id else { return }
        operation?.cancel()
        if let id {
            defaults.set(id, forKey: Self.selectedHostDefaultsKey)
        } else {
            defaults.removeObject(forKey: Self.selectedHostDefaultsKey)
        }
        let cached = id.flatMap { cache.hosts[$0] }
        state = makeState(
            selectedHostID: id,
            sessions: cached?.sessions ?? [],
            connection: id == nil ? .pairedOffline : .pairedOffline,
            cacheUpdatedAt: cached?.updatedAt,
            isCached: cached != nil
        )
        guard let id, let host = hostRecords.first(where: { $0.id == id }) else { return }
        cache.markViewed(hostFingerprint: id, at: .now)
        operation = Task { [weak self] in await self?.refresh(host: host) }
    }

    func retry() {
        guard let selectedHost else { return }
        operation?.cancel()
        operation = Task { [weak self] in await self?.refresh(host: selectedHost) }
    }

    @discardableResult
    func selectControllerRoute(
        _ target: ControllerRemoteRouteKind,
        explicitlyConfirmed: Bool
    ) -> Bool {
        let source = selectedRoute
        do {
            let plan = try routeCoordinator.select(
                target,
                explicitlyConfirmed: explicitlyConfirmed
            )
            routeSelectionError = nil
            defaults.set(target.rawValue, forKey: Self.selectedRouteDefaultsKey)
            syncRouteProjections()
            operation?.cancel()
            operation = nil
            activeTerminal?.suspend()
            activeTerminal = nil
            let cached = state.selectedHostID.flatMap { cache.hosts[$0] }
            state = makeState(
                selectedHostID: state.selectedHostID,
                sessions: cached?.sessions ?? state.sessions,
                connection: hostRecords.isEmpty ? .unpaired : .pairedOffline,
                cacheUpdatedAt: cached?.updatedAt,
                isCached: cached != nil
            )
            guard let host = selectedHost else { return true }
            operation = Task { [weak self] in
                guard let self else { return }
                if plan.disconnectTransport == source, let source {
                    await routeConnections.connection(for: source)?.cancel()
                }
                guard !Task.isCancelled else { return }
                await refresh(host: host)
            }
            return true
        } catch let error as AppleControllerRouteCoordinatorError {
            routeSelectionError = error
            syncRouteProjections()
            return false
        } catch {
            routeSelectionError = .transition(.invalidTransition)
            syncRouteProjections()
            return false
        }
    }

    func suspend() {
        activeTerminal?.suspend()
        operation?.cancel()
        operation = nil
        let connection = selectedConnection
        _ = try? routeCoordinator.cancelSelected()
        syncRouteProjections()
        Task { await connection?.cancel() }
        let cached = state.selectedHostID.flatMap { cache.hosts[$0] }
        state = makeState(
            selectedHostID: state.selectedHostID,
            sessions: cached?.sessions ?? state.sessions,
            connection: hostRecords.isEmpty ? .unpaired : .pairedOffline,
            cacheUpdatedAt: cached?.updatedAt,
            isCached: cached != nil
        )
    }

    func resume() {
        if let activeTerminal {
            activeTerminal.resume()
            return
        }
        guard operation == nil else { return }
        retry()
    }

    func openReadOnlyTerminal(_ session: ControllerSessionSummary) {
        guard activeTerminal == nil,
              !state.isCachedReadOnly,
              state.connection == .readyReadOnly,
              let host = selectedHost,
              let connectionActor = selectedConnection,
              host.capabilityBits & (1 << 1) != 0,
              session.capabilities.isEmpty || session.capabilities.contains(.attachOutput),
              session.occupantGeneration != nil else { return }
        activeTerminal = try? ControllerTerminalViewModel(
            host: host,
            session: session,
            connection: connectionActor
        )
    }

    func closeReadOnlyTerminal() {
        activeTerminal?.detach()
        activeTerminal = nil
        retry()
    }

    func forgetSelectedHost() {
        guard let host = selectedHost,
              let connectionActor = routeConnections.privateNetwork,
              let hostStore else { return }
        operation?.cancel()
        operation = Task { [weak self] in
            guard let self else { return }
            do {
                try await connectionActor.forgetDeviceSecret(host: host)
                hostRecords = try await hostStore.remove(id: host.id)
                cache.remove(hostFingerprint: host.id)
                if let cacheStore { try await cacheStore.save(cache) }
                let next = hostRecords.max { $0.pairedAt < $1.pairedAt }
                if let next {
                    defaults.set(next.id, forKey: Self.selectedHostDefaultsKey)
                } else {
                    defaults.removeObject(forKey: Self.selectedHostDefaultsKey)
                }
                state = makeState(
                    selectedHostID: next?.id,
                    sessions: next.flatMap { cache.hosts[$0.id]?.sessions } ?? [],
                    connection: next == nil ? .unpaired : .pairedOffline,
                    cacheUpdatedAt: next.flatMap { cache.hosts[$0.id]?.updatedAt },
                    isCached: next.flatMap { cache.hosts[$0.id] } != nil
                )
            } catch {
                state = replacing(connection: .failed(Self.failure(error)), sessions: state.sessions)
            }
        }
    }

    private func restore() async {
        guard let hostStore, let cacheStore else { return }
        do {
            async let loadedHosts = hostStore.load()
            async let loadedCache = cacheStore.load()
            hostRecords = try await loadedHosts
            cache = try await loadedCache
            let savedHostID = defaults.string(forKey: Self.selectedHostDefaultsKey)
            let selected = hostRecords.first(where: { $0.id == savedHostID })
                ?? hostRecords.max { $0.pairedAt < $1.pairedAt }
            if let selected {
                defaults.set(selected.id, forKey: Self.selectedHostDefaultsKey)
            }
            let cached = selected.flatMap { cache.hosts[$0.id] }
            state = makeState(
                selectedHostID: selected?.id,
                sessions: cached?.sessions ?? [],
                connection: selected == nil ? .unpaired : .pairedOffline,
                cacheUpdatedAt: cached?.updatedAt,
                isCached: cached != nil
            )
            if let selected { await refresh(host: selected) }
        } catch {
            state = replacing(connection: .failed(.storageUnavailable), sessions: [])
        }
    }

    private func refresh(host: PairedHostRecord) async {
        guard let route = selectedRoute,
              let connectionActor = routeConnections.connection(for: route),
              let cacheStore else {
            state = replacing(connection: .failed(.networkUnavailable), sessions: state.sessions)
            return
        }
        switch routePhase(route) {
        case .idle:
            _ = try? routeCoordinator.connectSelected()
        case .degraded:
            _ = try? routeCoordinator.retrySelected()
        case nil, .disabled, .unavailable, .revoked:
            state = replacing(connection: .failed(.networkUnavailable), sessions: state.sessions)
            syncRouteProjections()
            return
        case .connecting, .authenticating, .online, .reconnecting:
            break
        }
        syncRouteProjections()
        let startedAt = Date()
        var attempt = 1

        while !Task.isCancelled, state.selectedHostID == host.id {
            state = replacing(connection: .connecting, sessions: state.sessions)
            do {
                let snapshot = try await connectionActor.fetchSessions(host: host) { [weak self] progress in
                    await self?.apply(progress: progress, route: route, forHostID: host.id)
                }
                guard !Task.isCancelled, state.selectedHostID == host.id else { return }
                if host.capabilityBits != snapshot.capabilityBits, let hostStore {
                    let refreshedHost = try PairedHostRecord(
                        id: host.id,
                        displayName: host.displayName,
                        route: host.route,
                        hostStaticPublicKey: host.hostStaticPublicKey,
                        deviceStaticKeyId: host.deviceStaticKeyId,
                        deviceId: host.deviceId,
                        identityGeneration: host.identityGeneration,
                        revocationEpoch: host.revocationEpoch,
                        sessionGeneration: host.sessionGeneration,
                        capabilityBits: snapshot.capabilityBits,
                        pairedAt: host.pairedAt
                    )
                    hostRecords = try await hostStore.upsert(refreshedHost)
                }
                if routePhase(route) == .authenticating {
                    _ = try? routeCoordinator.authenticated(route)
                    syncRouteProjections()
                }
                if cache.hosts[host.id]?.revision != snapshot.revision
                    || cache.hosts[host.id]?.updateSequence != snapshot.updateSequence {
                    try cache.replace(
                        hostFingerprint: host.id,
                        revision: snapshot.revision,
                        updateSequence: snapshot.updateSequence,
                        sessions: snapshot.sessions,
                        selectedHostFingerprint: host.id,
                        now: .now
                    )
                    try await cacheStore.save(cache)
                }
                let stored = cache.hosts[host.id]
                state = makeState(
                    selectedHostID: host.id,
                    sessions: snapshot.sessions,
                    connection: .readyReadOnly,
                    cacheUpdatedAt: stored?.updatedAt,
                    isCached: false
                )
                return
            } catch {
                guard !Task.isCancelled, state.selectedHostID == host.id else { return }
                let elapsed = Date().timeIntervalSince(startedAt)
                let delay = Self.shouldRetry(error)
                    ? retryPolicy.delayAfterFailure(
                          attempt: attempt,
                          elapsedSeconds: elapsed
                      ) : nil
                if Self.isRevocation(error) {
                    _ = try? routeCoordinator.revokeSelected()
                } else {
                    _ = try? routeCoordinator.failed(
                        route,
                        retryable: delay != nil,
                        mutationInFlight: false
                    )
                }
                syncRouteProjections()
                guard let delay else {
                    applyTerminalFailure(error, for: host)
                    return
                }
                attempt += 1
                do {
                    try await retryPolicy.sleep(for: delay)
                } catch {
                    return
                }
            }
        }
    }

    private func apply(
        progress: ControllerConnectionProgress,
        route: ControllerRemoteRouteKind,
        forHostID hostID: String
    ) {
        guard !Task.isCancelled, state.selectedHostID == hostID else { return }
        let connection: ControllerConnectionState
        switch progress {
        case .authenticating:
            if routePhase(route) == .connecting || routePhase(route) == .reconnecting {
                _ = try? routeCoordinator.transportReady(route)
            }
            connection = .authenticating
        case .syncing:
            if routePhase(route) == .authenticating {
                _ = try? routeCoordinator.authenticated(route)
            }
            connection = .syncing
        }
        syncRouteProjections()
        state = replacing(connection: connection, sessions: state.sessions)
    }

    private func applyTerminalFailure(_ error: Error, for host: PairedHostRecord) {
        let cached = cache.hosts[host.id]
        let connection: ControllerConnectionState
        if case ControllerBindingError.IncompatibleVersion = error {
            connection = .incompatible
        } else if let error = error as? ControllerConnectionError,
                  case .hostError(let code) = error,
                  Self.isRevocationCode(code) {
            connection = .revoked
        } else {
            connection = .failed(Self.failure(error))
        }
        state = makeState(
            selectedHostID: host.id,
            sessions: cached?.sessions ?? [],
            connection: connection,
            cacheUpdatedAt: cached?.updatedAt,
            isCached: cached != nil
        )
    }

    private static func shouldRetry(_ error: Error) -> Bool {
        if error is CancellationError || error is SecureBlobError { return false }
        if let error = error as? ControllerPairingError {
            switch error {
            case .timedOut:
                return true
            case .invalidOffer, .expiredOrIncompatibleOffer, .publicRouteRejected,
                 .invalidDeviceName, .randomUnavailable, .frameTooLarge,
                 .connectionClosed, .cancelled, .noPairingInProgress, .rejected,
                 .hostIdentityChanged, .invalidAcknowledgement,
                 .acknowledgementUncertain:
                return false
            }
        }
        if let error = error as? ControllerFailure {
            switch error {
            case .networkUnavailable, .timedOut, .sequenceGap:
                return true
            case .cancelled, .invalidOffer, .offerExpired, .sasMismatch,
                 .authenticationFailed, .keychainUnavailable, .malformedResponse,
                 .resourceLimit, .storageUnavailable, .pairingUncertain:
                return false
            }
        }
        if let error = error as? ControllerConnectionError {
            switch error {
            case .authenticationFailed, .capabilityDenied, .malformedResponse, .resourceLimit:
                return false
            case .hostError(let code):
                return !isTerminalHostCode(code)
            case .sequenceGap:
                return true
            }
        }
        if let error = error as? ControllerBindingError {
            switch error {
            case .TimedOut, .Unexpected:
                return true
            default:
                return false
            }
        }
        return true
    }

    private static func isTerminalHostCode(_ code: String) -> Bool {
        let normalized = code.lowercased()
        return ["auth", "denied", "forbidden", "incompatible", "policy", "revoked", "unauthorized", "version"]
            .contains { normalized.contains($0) }
    }

    private static func isRevocationCode(_ code: String) -> Bool {
        code.lowercased().contains("revok")
    }

    private static func isRevocation(_ error: Error) -> Bool {
        guard let error = error as? ControllerConnectionError,
              case .hostError(let code) = error else { return false }
        return isRevocationCode(code)
    }

    private func routePhase(
        _ route: ControllerRemoteRouteKind
    ) -> ControllerRemoteRoutePhase? {
        routeCoordinator.projections.first { $0.route == route }?.phase
    }

    private func syncRouteProjections() {
        routeProjections = routeCoordinator.projections
    }

    private func replacing(
        connection: ControllerConnectionState,
        sessions: [ControllerSessionSummary]
    ) -> ControllerViewState {
        ControllerViewState(
            hosts: state.hosts,
            selectedHostID: state.selectedHostID,
            sessions: sessions,
            connection: connection,
            cacheUpdatedAt: state.cacheUpdatedAt,
            isCachedReadOnly: state.isCachedReadOnly
        )
    }

    private func makeState(
        selectedHostID: String?,
        sessions: [ControllerSessionSummary],
        connection: ControllerConnectionState,
        cacheUpdatedAt: Date?,
        isCached: Bool
    ) -> ControllerViewState {
        ControllerViewState(
            hosts: hostRecords.map {
                HostSummary(
                    id: $0.id,
                    title: $0.displayName,
                    route: $0.route,
                    fingerprint: $0.fingerprint,
                    capabilityBits: $0.capabilityBits
                )
            },
            selectedHostID: selectedHostID,
            sessions: sessions,
            connection: connection,
            cacheUpdatedAt: cacheUpdatedAt,
            isCachedReadOnly: isCached
        )
    }

    private static func failure(_ error: Error) -> ControllerFailure {
        if error is CancellationError { return .cancelled }
        if let error = error as? ControllerFailure { return error }
        if let error = error as? ControllerPairingError {
            switch error {
            case .expiredOrIncompatibleOffer: return .offerExpired
            case .invalidOffer, .publicRouteRejected, .invalidDeviceName: return .invalidOffer
            case .timedOut: return .timedOut
            case .rejected: return .sasMismatch
            case .randomUnavailable: return .keychainUnavailable
            case .acknowledgementUncertain: return .pairingUncertain
            default: return .authenticationFailed
            }
        }
        if let error = error as? ControllerConnectionError {
            switch error {
            case .sequenceGap: return .sequenceGap
            case .resourceLimit: return .resourceLimit
            case .malformedResponse: return .malformedResponse
            case .authenticationFailed, .capabilityDenied, .hostError(_): return .authenticationFailed
            }
        }
        if error is SecureBlobError { return .keychainUnavailable }
        if error is ControllerCacheError || error is PairedHostStoreError {
            return .storageUnavailable
        }
        return .networkUnavailable
    }

    private static func loadDeviceID(defaults: UserDefaults) -> UUID {
        let key = "termirust.controller.device_uuid"
        if let value = defaults.string(forKey: key), let id = UUID(uuidString: value) {
            return id
        }
        let id = UUID()
        defaults.set(id.uuidString.lowercased(), forKey: key)
        return id
    }

    private static let selectedRouteDefaultsKey = "termirust.controller.selected_route.v1"
    private static let selectedHostDefaultsKey = "termirust.controller.selected_host.v1"
}
