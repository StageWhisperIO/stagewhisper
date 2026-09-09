import AVFoundation
import Foundation

@_cdecl("sw_microphone_auth_status")
func swMicrophoneAuthStatus() -> Int32 {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .notDetermined: return 0
    case .restricted: return 1
    case .denied: return 2
    case .authorized: return 3
    @unknown default: return 0
    }
}

@_cdecl("sw_microphone_request_access")
func swMicrophoneRequestAccess() -> Bool {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false

    DispatchQueue.global(qos: .userInitiated).async {
        AVCaptureDevice.requestAccess(for: .audio) { success in
            granted = success
            semaphore.signal()
        }
    }

    semaphore.wait()
    return granted
}
