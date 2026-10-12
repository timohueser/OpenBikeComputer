import AVFoundation
import SwiftUI

/// Camera preview that reports the text of every QR code it reads, on the main queue.
struct ScannerView: UIViewRepresentable {
    let onCode: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        view.previewLayer.session = context.coordinator.session
        view.previewLayer.videoGravity = .resizeAspectFill
        context.coordinator.start()
        return view
    }

    func updateUIView(_ uiView: PreviewView, context: Context) {}

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        let session = AVCaptureSession()
        private let onCode: (String) -> Void

        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }

        deinit { session.stopRunning() }

        func start() {
            AVCaptureDevice.requestAccess(for: .video) { [self] granted in
                if granted { configureAndRun() }
            }
        }

        private func configureAndRun() {
            // The virtual multi-camera devices switch to the ultra wide lens for close-up focus.
            let types: [AVCaptureDevice.DeviceType] = [.builtInTripleCamera, .builtInDualWideCamera, .builtInWideAngleCamera]
            guard let device = AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: .video, position: .back).devices.first,
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input)
            else { return }
            let output = AVCaptureMetadataOutput()
            session.beginConfiguration()
            session.sessionPreset = .hd1920x1080
            session.addInput(input)
            guard session.canAddOutput(output) else {
                session.commitConfiguration()
                return
            }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
            session.commitConfiguration()
            if (try? device.lockForConfiguration()) != nil {
                if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
                if device.isAutoFocusRangeRestrictionSupported { device.autoFocusRangeRestriction = .near }
                device.unlockForConfiguration()
            }
            session.startRunning()
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
            for case let code as AVMetadataMachineReadableCodeObject in metadataObjects {
                if let text = code.stringValue { onCode(text) }
            }
        }
    }
}
