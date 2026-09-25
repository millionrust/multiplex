import SwiftUI

@main
struct MultiplexMobileApp: App {
    @StateObject private var connectionViewModel: HostListViewModel = {
        let secretStore = KeychainSecretStore(service: "com.multiplex.mobile.ssh")
        let identity = UserDefaultsMobileDeviceIdentityStore()
        return HostListViewModel(
            vaultImporter: MobileVaultImporter(decryptor: NativeMobileVaultDecryptor()),
            secretStore: secretStore,
            encryptedVaultStore: try? FileEncryptedVaultStore(),
            sshClient: DirectSSHSessionClient(secretStore: secretStore),
            localDeviceId: identity.deviceId
        )
    }()
    @StateObject private var controllerViewModel = ControllerViewModel()

    var body: some Scene {
        WindowGroup {
            UnifiedMobileRootView(
                connectionViewModel: connectionViewModel,
                controllerViewModel: controllerViewModel
            )
        }
    }
}

private struct UnifiedMobileRootView: View {
    @ObservedObject var connectionViewModel: HostListViewModel
    @ObservedObject var controllerViewModel: ControllerViewModel
    @Environment(\.scenePhase) private var scenePhase
    /// The phone opens on the computers it is paired with, which is what it is for: the SSH
    /// connections it keeps itself are the other tab, not the front door.
    @State private var destination = MobileRootDestination.devices

    var body: some View {
        TabView(selection: $destination) {
            ContentView(viewModel: connectionViewModel)
                .tabItem { Label("Connections", systemImage: "terminal") }
                .tag(MobileRootDestination.connections)
            ControllerRootView(viewModel: controllerViewModel)
                .tabItem { Label("Devices", systemImage: "macbook.and.iphone") }
                .tag(MobileRootDestination.devices)
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active:
                connectionViewModel.resume()
                controllerViewModel.resume()
            case .background, .inactive:
                connectionViewModel.suspend()
                controllerViewModel.suspend()
            @unknown default:
                connectionViewModel.suspend()
                controllerViewModel.suspend()
            }
        }
    }
}
