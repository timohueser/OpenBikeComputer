import OBCHost
import SwiftUI
import UIKit

/// The four buttons as touch pads. One view takes every touch: SwiftUI's gestures do not deliver
/// two concurrent presses on two views, and the chords (Up+Select, Down+Back) need exactly that.
///
/// A touch belongs to the pad it began on until it lifts or cancels — a finger that slides off a
/// pad has not let go of the button. A cancelled touch is a release.
final class PadsView: UIView {
    /// The panel's own order: Up over Down on the left, Select over Back on the right.
    private static let pads: [(button: ObcButton, title: String, column: CGFloat, row: CGFloat)] = [
        (OBC_BUTTON_UP, "Up", 0, 0),
        (OBC_BUTTON_DOWN, "Down", 0, 1),
        (OBC_BUTTON_SELECT, "Select", 1, 0),
        (OBC_BUTTON_BACK, "Back", 1, 1),
    ]
    private static let gap: CGFloat = 10
    private static let idle = UIColor(white: 0.16, alpha: 1)
    private static let down = UIColor(white: 0.42, alpha: 1)

    /// Where a pad's edges go.
    var edge: ((ObcButton, Bool) -> Void)?

    private let keys: [UIView]
    /// Which pad each live touch began on.
    private var holds: [ObjectIdentifier: Int] = [:]
    private let haptic = UIImpactFeedbackGenerator(style: .light)

    override init(frame: CGRect) {
        keys = Self.pads.map { pad in
            let key = UIView()
            key.backgroundColor = Self.idle
            key.layer.cornerRadius = 20
            let title = UILabel()
            title.text = pad.title
            title.font = .systemFont(ofSize: 22, weight: .semibold)
            title.textColor = .white
            title.translatesAutoresizingMaskIntoConstraints = false
            key.addSubview(title)
            NSLayoutConstraint.activate([
                title.centerXAnchor.constraint(equalTo: key.centerXAnchor),
                title.centerYAnchor.constraint(equalTo: key.centerYAnchor),
            ])
            return key
        }
        super.init(frame: frame)
        isMultipleTouchEnabled = true
        keys.forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("PadsView is made in code") }

    override func layoutSubviews() {
        super.layoutSubviews()
        let width = (bounds.width - Self.gap) / 2
        let height = (bounds.height - Self.gap) / 2
        for (key, pad) in zip(keys, Self.pads) {
            key.frame = CGRect(
                x: pad.column * (width + Self.gap), y: pad.row * (height + Self.gap),
                width: width, height: height)
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            guard let index = keys.firstIndex(where: { $0.frame.contains(touch.location(in: self)) }) else { continue }
            holds[ObjectIdentifier(touch)] = index
            settle(index, down: true)
        }
    }

    /// A moved touch keeps its pad, so there is nothing to do — and nothing to forward either.
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {}

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { release(touches) }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { release(touches) }

    private func release(_ touches: Set<UITouch>) {
        for touch in touches {
            guard let index = holds.removeValue(forKey: ObjectIdentifier(touch)) else { continue }
            settle(index, down: false)
        }
    }

    /// Report the pad's held state, not the touch's: a second finger on the same pad must not
    /// press it twice, and lifting one of them must not release it.
    private func settle(_ index: Int, down: Bool) {
        let fingers = holds.values.count(where: { $0 == index })
        guard fingers == (down ? 1 : 0) else { return }
        keys[index].backgroundColor = down ? Self.down : Self.idle
        if down { haptic.impactOccurred() }
        edge?(Self.pads[index].button, down)
    }
}

/// The pads in SwiftUI.
struct Pads: UIViewRepresentable {
    let controller: HostController

    func makeUIView(context: Context) -> PadsView {
        let view = PadsView()
        view.edge = { button, down in controller.button(button, down: down) }
        return view
    }

    func updateUIView(_ view: PadsView, context: Context) {}
}
