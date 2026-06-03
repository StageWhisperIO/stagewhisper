import AppKit
import SwiftUI

@available(macOS 14, *)
@Observable
final class ControlBarState {
    var isListening = false
    var isConnected = false
}

@available(macOS 14, *)
private let sharedState = ControlBarState()

private var onListenToggle: (@convention(c) () -> Void)?
private var onSettingsOpen: (@convention(c) () -> Void)?
private var onConnectAI: (@convention(c) () -> Void)?

private weak var controlBarNSView: NSView?

// MARK: - Glass Control Bar (macOS 26+)

@available(macOS 26, *)
struct GlassControlBarView: View {
    var state: ControlBarState
    @Namespace private var morphNamespace

    private var listenTitle: String {
        state.isListening ? "Stop" : "Listen"
    }

    private var listenIcon: String {
        state.isListening ? "stop.fill" : "waveform"
    }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(spacing: 10) {
                if state.isConnected {
                    Button {
                        onListenToggle?()
                    } label: {
                        Label(listenTitle, systemImage: listenIcon)
                            .font(.system(size: 12, weight: .semibold))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                    }
                    .buttonStyle(.glass)
                    .glassEffectID("listen", in: morphNamespace)
                } else {
                    Button {
                        onConnectAI?()
                    } label: {
                        Label("Connect AI", systemImage: "link")
                            .font(.system(size: 12, weight: .semibold))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                    }
                    .buttonStyle(.glass)
                    .glassEffectID("listen", in: morphNamespace)
                }

                HStack(spacing: 4) {
                    Text("On/Off-screen")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                    Text("⌘\\")
                        .font(.system(size: 10, weight: .medium, design: .rounded))
                        .padding(.horizontal, 5)
                        .padding(.vertical, 2)
                        .background(.white.opacity(0.08), in: .rect(cornerRadius: 4))
                        .foregroundStyle(.white.opacity(0.5))
                }

                Button {
                    onSettingsOpen?()
                } label: {
                    Image(systemName: "gearshape.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 4)
                }
                .buttonStyle(.glass)
                .glassEffectID("settings", in: morphNamespace)
            }
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(.smooth(duration: 0.35), value: state.isListening)
        .animation(.smooth(duration: 0.35), value: state.isConnected)
    }
}

// MARK: - Fallback Control Bar (macOS 14–25)

@available(macOS 14, *)
struct FallbackControlBarView: View {
    var state: ControlBarState

    private var listenTitle: String {
        state.isListening ? "Stop" : "Listen"
    }

    private var listenIcon: String {
        state.isListening ? "stop.fill" : "waveform"
    }

    var body: some View {
        HStack(spacing: 10) {
            if state.isConnected {
                Button {
                    onListenToggle?()
                } label: {
                    Label(listenTitle, systemImage: listenIcon)
                        .font(.system(size: 12, weight: .semibold))
                }
                .buttonStyle(.accessoryBar)
            } else {
                Button {
                    onConnectAI?()
                } label: {
                    Label("Connect AI", systemImage: "link")
                        .font(.system(size: 12, weight: .semibold))
                }
                .buttonStyle(.accessoryBar)
            }

            HStack(spacing: 4) {
                Text("On/Off-screen")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Text("⌘\\")
                    .font(.system(size: 10, weight: .medium, design: .rounded))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(.white.opacity(0.08), in: .rect(cornerRadius: 4))
                    .foregroundStyle(.white.opacity(0.5))
            }

            Button {
                onSettingsOpen?()
            } label: {
                Image(systemName: "gearshape.fill")
                    .font(.system(size: 11, weight: .semibold))
            }
            .buttonStyle(.accessoryBar)
        }
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        .animation(.easeInOut(duration: 0.25), value: state.isListening)
        .animation(.easeInOut(duration: 0.25), value: state.isConnected)
    }
}

// MARK: - Draggable Hosting View

@available(macOS 14, *)
class DraggableHostingView<T: View>: NSHostingView<T> {
    override var mouseDownCanMoveWindow: Bool { true }
}

// MARK: - FFI Exports (called from Rust via @_cdecl)

@_cdecl("sw_glass_create_control_bar")
func createControlBar() -> UnsafeMutableRawPointer? {
    if #available(macOS 26, *) {
        let view = GlassControlBarView(state: sharedState)
        let hostingView = DraggableHostingView(rootView: view)
        hostingView.translatesAutoresizingMaskIntoConstraints = false
        hostingView.appearance = NSAppearance(named: .vibrantDark)
        controlBarNSView = hostingView

        return Unmanaged.passRetained(hostingView).toOpaque()
    } else if #available(macOS 14, *) {
        let view = FallbackControlBarView(state: sharedState)
        let hostingView = DraggableHostingView(rootView: view)
        hostingView.translatesAutoresizingMaskIntoConstraints = false
        hostingView.appearance = NSAppearance(named: .vibrantDark)
        controlBarNSView = hostingView

        return Unmanaged.passRetained(hostingView).toOpaque()
    } else {
        return nil
    }
}

@_cdecl("sw_glass_update_state")
func updateState(isListening: Bool, isConnected: Bool) {
    let apply = {
        guard #available(macOS 14, *) else { return }
        sharedState.isListening = isListening
        sharedState.isConnected = isConnected
        controlBarNSView?.needsDisplay = true
        controlBarNSView?.window?.viewsNeedDisplay = true
        controlBarNSView?.window?.displayIfNeeded()
    }

    if Thread.isMainThread {
        apply()
    } else {
        DispatchQueue.main.async { apply() }
    }
}

@_cdecl("sw_glass_set_callbacks")
func setCallbacks(
    listen: @convention(c) @escaping () -> Void,
    settings: @convention(c) @escaping () -> Void,
    connectAI: @convention(c) @escaping () -> Void
) {
    onListenToggle = listen
    onSettingsOpen = settings
    onConnectAI = connectAI
}
