@preconcurrency import AVFoundation
import SwiftUI
import UIKit

struct ControllerQRCodeScanner: UIViewControllerRepresentable {
    let onCode: @MainActor (String) -> Void
    let onFailure: @MainActor (ControllerScannerFailure) -> Void

    func makeUIViewController(context: Context) -> ControllerScannerViewController {
        let controller = ControllerScannerViewController()
        controller.onCode = onCode
        controller.onFailure = onFailure
        return controller
    }

    func updateUIViewController(
        _ uiViewController: ControllerScannerViewController,
        context: Context
    ) {}

    static func dismantleUIViewController(
        _ uiViewController: ControllerScannerViewController,
        coordinator: Void
    ) {
        uiViewController.stop()
    }
}

@MainActor
final class ControllerScannerViewController: UIViewController,
    @preconcurrency AVCaptureMetadataOutputObjectsDelegate
{
    var onCode: (@MainActor (String) -> Void)?
    var onFailure: (@MainActor (ControllerScannerFailure) -> Void)?

    private let session = AVCaptureSession()
    private let preview = AVCaptureVideoPreviewLayer()
    private var deliveredResult = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        preview.session = session
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        configureForCurrentAuthorization()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview.frame = view.bounds
    }

    func stop() {
        guard session.isRunning else { return }
        Task.detached { [session] in session.stopRunning() }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !deliveredResult,
              let code = metadataObjects
                .compactMap({ $0 as? AVMetadataMachineReadableCodeObject })
                .first(where: { $0.type == .qr })?.stringValue,
              !code.isEmpty,
              code.utf8.count <= 4 * 1_024 else {
            return
        }
        deliveredResult = true
        stop()
        onCode?(code)
    }

    private func configureForCurrentAuthorization() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configureSession()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                Task { @MainActor in
                    guard let self else { return }
                    granted ? self.configureSession() : self.onFailure?(.permissionDenied)
                }
            }
        case .denied, .restricted:
            onFailure?(.permissionDenied)
        @unknown default:
            onFailure?(.unavailable)
        }
    }

    private func configureSession() {
        guard !session.isRunning, session.inputs.isEmpty else { return }
        guard let camera = AVCaptureDevice.default(for: .video) else {
            onFailure?(.unavailable)
            return
        }
        do {
            let input = try AVCaptureDeviceInput(device: camera)
            guard session.canAddInput(input) else {
                onFailure?(.unavailable)
                return
            }
            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else {
                onFailure?(.unavailable)
                return
            }
            session.addInput(input)
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
            Task.detached { [session] in session.startRunning() }
        } catch {
            onFailure?(.unavailable)
        }
    }
}

enum ControllerScannerFailure: Error, Equatable {
    case permissionDenied
    case unavailable
}
