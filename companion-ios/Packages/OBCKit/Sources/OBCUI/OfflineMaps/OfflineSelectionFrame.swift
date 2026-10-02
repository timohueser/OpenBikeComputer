#if os(iOS)
import SwiftUI
import UIKit

/// The map owns every touch. Only a one-finger drag that starts on a corner resizes the area.
@MainActor final class OfflineSelectionFrame: UIView, UIGestureRecognizerDelegate {
    var onResize: ((OfflineAreaSelection.Corner, CGPoint) -> Void)?
    private let outline = CAShapeLayer()
    private let coverage = CAShapeLayer()
    private var handles: [Handle] = []
    private var dragged: Handle?
    private var dragOrigin = CGPoint.zero

    override init(frame: CGRect) {
        super.init(frame: frame)
        layer.addSublayer(coverage)
        layer.addSublayer(outline)
        outline.fillColor = UIColor.clear.cgColor
        outline.lineWidth = 2
        outline.lineDashPattern = [6, 4]
        coverage.lineWidth = 1.5
        for corner in OfflineAreaSelection.Corner.allCases {
            let handle = Handle(corner: corner)
            handle.adjust = { [weak self, weak handle] amount in
                guard let self, let handle else { return }
                let direction: CGFloat = corner == .northwest ? -1 : 1
                onResize?(corner, CGPoint(x: handle.center.x + direction * amount, y: handle.center.y + direction * amount))
            }
            addSubview(handle)
            handles.append(handle)
        }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: OfflineSelectionFrame, _) in view.updateColors() }
        updateColors()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func attach(to map: UIView) {
        map.addSubview(self)
        let resize = UIPanGestureRecognizer(target: self, action: #selector(resizeGesture(_:)))
        resize.maximumNumberOfTouches = 1
        resize.delegate = self
        for gesture in map.gestureRecognizers ?? [] where gesture is UIPanGestureRecognizer {
            gesture.require(toFail: resize)
        }
        map.addGestureRecognizer(resize)
    }

    func show(selection: CGRect?, coverage: CGRect?, editable: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        outline.path = selection.map { UIBezierPath(rect: $0).cgPath }
        self.coverage.path = coverage.map { UIBezierPath(rect: $0).cgPath }
        outline.lineDashPattern = editable ? [6, 4] : nil
        for handle in handles {
            handle.isHidden = !editable || selection == nil
            guard let selection else { continue }
            let point = handle.corner == .northwest
                ? CGPoint(x: selection.minX, y: selection.minY) : CGPoint(x: selection.maxX, y: selection.maxY)
            handle.frame = CGRect(x: point.x - 36, y: point.y - 36, width: 72, height: 72)
        }
        CATransaction.commit()
    }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? { nil }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        guard gestureRecognizer.numberOfTouches == 0 else { return true }
        dragged = isHidden ? nil : corner(at: touch.location(in: self))
        dragOrigin = dragged?.center ?? .zero
        return dragged != nil
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer) -> Bool {
        otherGestureRecognizer is UIPinchGestureRecognizer
    }

    private func corner(at point: CGPoint) -> Handle? {
        handles.filter { !$0.isHidden && $0.frame.contains(point) }.min {
            hypot($0.center.x - point.x, $0.center.y - point.y) < hypot($1.center.x - point.x, $1.center.y - point.y)
        }
    }

    @objc private func resizeGesture(_ gesture: UIPanGestureRecognizer) {
        let translation = gesture.translation(in: self)
        if gesture.state == .began || gesture.state == .changed || gesture.state == .ended, let dragged {
            onResize?(dragged.corner, CGPoint(x: dragOrigin.x + translation.x, y: dragOrigin.y + translation.y))
        }
        if gesture.state == .ended || gesture.state == .cancelled || gesture.state == .failed { dragged = nil }
    }

    private func updateColors() {
        outline.strokeColor = UIColor(OBCTheme.ink).resolvedColor(with: traitCollection).cgColor
        let tint = UIColor(OBCTheme.tint).resolvedColor(with: traitCollection)
        coverage.fillColor = tint.withAlphaComponent(0.10).cgColor
        coverage.strokeColor = tint.withAlphaComponent(0.7).cgColor
        handles.forEach { $0.setNeedsDisplay() }
    }

    private final class Handle: UIView {
        let corner: OfflineAreaSelection.Corner
        var adjust: ((CGFloat) -> Void)?
        init(corner: OfflineAreaSelection.Corner) {
            self.corner = corner
            super.init(frame: .zero)
            isOpaque = false
            isAccessibilityElement = true
            accessibilityTraits = .adjustable
            accessibilityLabel = corner == .northwest ? "Northwest corner" : "Southeast corner"
            accessibilityHint = "Swipe up to expand the area, or down to make it smaller."
            accessibilityIdentifier = corner == .northwest ? "offline.resize.northwest" : "offline.resize.southeast"
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func accessibilityIncrement() { adjust?(12) }
        override func accessibilityDecrement() { adjust?(-12) }
        override func draw(_ rect: CGRect) {
            let shape = UIBezierPath(ovalIn: CGRect(x: bounds.midX - 12, y: bounds.midY - 12, width: 24, height: 24))
            UIColor(OBCTheme.surface).setFill(); shape.fill()
            UIColor(OBCTheme.ink).setStroke(); shape.lineWidth = 2; shape.stroke()
        }
    }
}
#endif
