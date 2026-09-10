import Foundation
import SwiftUI

@MainActor
final class EnrollmentViewModel: ObservableObject {
    @Published private(set) var snapshot = EnrollmentSnapshot()
    @Published private(set) var loading = false
    @Published private(set) var problem: LocalizedStringKey?
    private let repository: any EnrollmentRepository

    init(repository: any EnrollmentRepository = NativeEnrollmentRepository()) {
        self.repository = repository
    }

    func reload() async { await operate { try await self.repository.load() } }
    func prepare() async {
        guard problem == nil, snapshot.request == nil else { return }
        await operate { try await self.repository.prepare() }
    }
    func cancel(_ reviewed: Data) async {
        guard !loading else { return }
        snapshot = EnrollmentSnapshot(request: reviewed, cancellationPending: true)
        await operate { try await self.repository.cancel(reviewed) }
    }

    private func operate(_ action: () async throws -> EnrollmentSnapshot) async {
        guard !loading else { return }
        loading = true
        problem = nil
        defer { loading = false }
        do { snapshot = try await action() }
        catch {
            switch error {
            case MobileReplicationError.Locked, ReplicationStorageError.Locked:
                problem = "Unlock your device, then reload or retry cancellation."
            case MobileReplicationError.Busy:
                problem = "Another operation is in progress. Reload to check the saved request."
            case MobileReplicationError.Invalid:
                problem = "Stored enrollment data is invalid. It has not been replaced."
            case MobileReplicationError.StaleRequest:
                problem = "The saved request changed. Reload before continuing."
            case MobileReplicationError.RecoveryRequired:
                problem = "Enrollment recovery is required. Existing data has been preserved."
            case MobileReplicationError.AlreadyConfigured:
                problem = "Replication is already configured. Preparation is unavailable here."
            default:
                problem = "Enrollment storage is unavailable. Reload before trying again."
            }
        }
    }
}
