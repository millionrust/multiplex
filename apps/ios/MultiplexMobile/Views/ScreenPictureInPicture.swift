@preconcurrency import AVKit
import Combine
import CoreMedia
import CoreVideo
import SwiftUI
import UIKit

@MainActor
final class ScreenPictureInPicture: NSObject, ObservableObject,
    AVPictureInPictureControllerDelegate, AVPictureInPictureSampleBufferPlaybackDelegate {
    @Published private(set) var active = false
    @Published private(set) var starting = false
    @Published private(set) var failure: String?
    var onEnd: (() -> Void)?
    private var controller: AVPictureInPictureController?

    var supported: Bool { AVPictureInPictureController.isPictureInPictureSupported() }
    var possible: Bool { controller?.isPictureInPicturePossible == true }

    func configure(layer: AVSampleBufferDisplayLayer) {
        guard supported, controller == nil else { return }
        let source = AVPictureInPictureController.ContentSource(sampleBufferDisplayLayer: layer, playbackDelegate: self)
        let controller = AVPictureInPictureController(contentSource: source)
        controller.delegate = self
        controller.canStartPictureInPictureAutomaticallyFromInline = true
        controller.requiresLinearPlayback = true
        self.controller = controller
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback, options: .mixWithOthers)
        try? AVAudioSession.sharedInstance().setActive(true)
    }

    @discardableResult
    func start() -> Bool {
        if controller?.isPictureInPictureActive == true { active = true; return true }
        guard possible else { return active || starting }
        guard !active, !starting else { return true }
        starting = true
        failure = nil
        controller?.startPictureInPicture()
        return true
    }

    func stop() {
        controller?.stopPictureInPicture()
        starting = false
    }

    nonisolated func pictureInPictureControllerWillStartPictureInPicture(_ controller: AVPictureInPictureController) {
        Task { @MainActor in self.starting = true }
    }
    nonisolated func pictureInPictureControllerDidStartPictureInPicture(_ controller: AVPictureInPictureController) {
        Task { @MainActor in self.starting = false; self.active = true }
    }
    nonisolated func pictureInPictureControllerDidStopPictureInPicture(_ controller: AVPictureInPictureController) {
        Task { @MainActor in
            self.active = false
            self.starting = false
            self.onEnd?()
        }
    }
    nonisolated func pictureInPictureController(_ controller: AVPictureInPictureController, failedToStartPictureInPictureWithError error: Error) {
        Task { @MainActor in
            self.active = false
            self.starting = false
            self.failure = "Picture in Picture could not start."
            self.onEnd?()
        }
    }
    nonisolated func pictureInPictureController(_ controller: AVPictureInPictureController, restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completion: @escaping (Bool) -> Void) {
        completion(true) // The same computer's viewer stays presented beneath PiP.
    }
    nonisolated func pictureInPictureController(_ controller: AVPictureInPictureController, setPlaying playing: Bool) {}
    nonisolated func pictureInPictureControllerTimeRangeForPlayback(_ controller: AVPictureInPictureController) -> CMTimeRange {
        CMTimeRange(start: .zero, duration: .positiveInfinity)
    }
    nonisolated func pictureInPictureControllerIsPlaybackPaused(_ controller: AVPictureInPictureController) -> Bool { false }
    nonisolated func pictureInPictureController(_ controller: AVPictureInPictureController, didTransitionToRenderSize size: CMVideoDimensions) {}
    nonisolated func pictureInPictureController(_ controller: AVPictureInPictureController, skipByInterval interval: CMTime, completion: @escaping () -> Void) { completion() }

}

/// The same live display layer draws the inline picture and the system PiP window.
struct SharedScreenPicture: UIViewRepresentable {
    let image: CGImage
    let model: RemoteScreenViewModel

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> SharedScreenPictureSurface {
        let view = SharedScreenPictureSurface()
        model.pictureInPicture.configure(layer: view.videoLayer)
        context.coordinator.bind(model: model, surface: view)
        return view
    }

    func updateUIView(_ view: SharedScreenPictureSurface, context: Context) { view.display(image) }

    @MainActor
    final class Coordinator {
        private var frames: AnyCancellable?
        func bind(model: RemoteScreenViewModel, surface: SharedScreenPictureSurface) {
            // Feed PiP directly: SwiftUI may stop drawing its inline view in the background.
            frames = model.$image.sink { [weak surface] image in
                if let image { surface?.display(image) }
            }
        }
    }
}

final class SharedScreenPictureSurface: UIView {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }
    var videoLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }
    private var lastImage: CGImage?

    override init(frame: CGRect) {
        super.init(frame: frame)
        videoLayer.videoGravity = .resizeAspect
        backgroundColor = .black
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func display(_ image: CGImage) {
        guard lastImage !== image, let sample = Self.sampleBuffer(image: image) else { return }
        if videoLayer.status == .failed { videoLayer.flush() }
        guard videoLayer.isReadyForMoreMediaData else { return }
        lastImage = image
        videoLayer.enqueue(sample)
    }

    static func sampleBuffer(image: CGImage) -> CMSampleBuffer? {
        guard image.width > 0, image.height > 0, image.width <= 8192, image.height <= 8192, image.width * image.height <= 8_388_608 else { return nil }
        var buffer: CVPixelBuffer?
        let attributes: [CFString: Any] = [
            kCVPixelBufferCGImageCompatibilityKey: true,
            kCVPixelBufferCGBitmapContextCompatibilityKey: true,
            kCVPixelBufferMetalCompatibilityKey: true,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as [String: Any],
        ]
        guard CVPixelBufferCreate(kCFAllocatorDefault, image.width, image.height,
                                  kCVPixelFormatType_32BGRA, attributes as CFDictionary, &buffer) == kCVReturnSuccess,
              let buffer else { return nil }
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let context = CGContext(data: CVPixelBufferGetBaseAddress(buffer), width: image.width,
                                      height: image.height, bitsPerComponent: 8,
                                      bytesPerRow: CVPixelBufferGetBytesPerRow(buffer),
                                      space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        var format: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault,
                                                           imageBuffer: buffer, formatDescriptionOut: &format) == noErr,
              let format else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid,
                                       presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
                                       decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault,
            imageBuffer: buffer, formatDescription: format, sampleTiming: &timing, sampleBufferOut: &sample) == noErr,
              let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) {
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dictionary, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }
}
