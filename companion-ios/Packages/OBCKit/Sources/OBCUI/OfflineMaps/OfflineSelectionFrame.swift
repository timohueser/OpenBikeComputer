#if os(iOS)
import SwiftUI
import UIKit

/// Only the resize handles consume touches. The map keeps its pan and zoom gestures.
@MainActor final class OfflineSelectionFrame: UIView {
    var onChange: ((CGRect) -> Void)?
    var coverageRects: [CGRect] = [] { didSet { render() } }
    private(set) var selection = CGRect.zero
    private var previousBounds = CGRect.zero
    private var dragStart = CGRect.zero
    private let shade = CAShapeLayer()
    private let outline = CAShapeLayer()
    private let coverage = CAShapeLayer()
    private var handles: [Handle] = []

    override init(frame: CGRect) {
        super.init(frame: frame)
        layer.addSublayer(shade); layer.addSublayer(coverage); layer.addSublayer(outline)
        coverage.lineWidth = 1
        shade.fillRule = .evenOdd
        shade.fillColor = UIColor.black.withAlphaComponent(0.25).cgColor
        outline.fillColor = UIColor.clear.cgColor
        outline.lineWidth = 2
        for (edges, label) in [(1, "West edge"), (2, "East edge"), (4, "North edge"), (8, "South edge"),
                               (5, "Northwest corner"), (6, "Northeast corner"), (9, "Southwest corner"), (10, "Southeast corner")] {
            let handle = Handle()
            handle.isOpaque = false
            handle.tag = edges; handle.accessibilityLabel = label
            handle.accessibilityIdentifier = "offline.resize.\(edges)"
            handle.isAccessibilityElement = true; handle.accessibilityTraits = .adjustable
            handle.adjust = { [weak self] amount in
                guard let self else { return }
                dragStart = selection
                resize(edges, by: CGPoint(x: amount, y: amount))
            }
            handle.addGestureRecognizer(UIPanGestureRecognizer(target: self, action: #selector(resizeGesture(_:))))
            addSubview(handle); handles.append(handle)
        }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        if previousBounds != bounds, bounds.width > 0, bounds.height > 0 {
            if previousBounds.isEmpty { selection = bounds.insetBy(dx: 44, dy: 52) }
            else {
                selection = CGRect(x: selection.minX * bounds.width / previousBounds.width,
                    y: selection.minY * bounds.height / previousBounds.height,
                    width: selection.width * bounds.width / previousBounds.width,
                    height: selection.height * bounds.height / previousBounds.height)
            }
            previousBounds = bounds
            onChange?(selection)
        }
        render()
    }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        guard !isHidden, isUserInteractionEnabled else { return nil }
        return handles.first { $0.frame.contains(point) }
    }

    @objc private func resizeGesture(_ gesture: UIPanGestureRecognizer) {
        if gesture.state == .began { dragStart = selection }
        resize(gesture.view!.tag, by: gesture.translation(in: self))
    }

    private func resize(_ edges: Int, by delta: CGPoint) {
        var left = dragStart.minX, right = dragStart.maxX, top = dragStart.minY, bottom = dragStart.maxY
        let minimum: CGFloat = 88
        if edges & 1 != 0 { left = min(right - minimum, max(22, left + delta.x)) }
        if edges & 2 != 0 { right = max(left + minimum, min(bounds.width - 22, right + delta.x)) }
        if edges & 4 != 0 { top = min(bottom - minimum, max(22, top + delta.y)) }
        if edges & 8 != 0 { bottom = max(top + minimum, min(bounds.height - 22, bottom + delta.y)) }
        selection = CGRect(x: left, y: top, width: right - left, height: bottom - top)
        render(); onChange?(selection)
    }

    private func render() {
        let path = UIBezierPath(rect: bounds); path.append(UIBezierPath(rect: selection))
        shade.path = path.cgPath; outline.path = UIBezierPath(rect: selection).cgPath
        outline.strokeColor = UIColor(OBCTheme.ink).resolvedColor(with: traitCollection).cgColor
        let cells = UIBezierPath()
        for rect in coverageRects { cells.append(UIBezierPath(rect: rect)) }
        coverage.path = cells.cgPath
        let tint = UIColor(OBCTheme.tint).resolvedColor(with: traitCollection)
        coverage.fillColor = tint.withAlphaComponent(0.12).cgColor
        coverage.strokeColor = tint.withAlphaComponent(0.7).cgColor
        for handle in handles {
            let edges = handle.tag
            let x = edges & 1 != 0 ? selection.minX : edges & 2 != 0 ? selection.maxX : selection.midX
            let y = edges & 4 != 0 ? selection.minY : edges & 8 != 0 ? selection.maxY : selection.midY
            handle.frame = CGRect(x: x - 22, y: y - 22, width: 44, height: 44)
            handle.setNeedsDisplay()
        }
    }

    private final class Handle: UIView {
        var adjust: ((CGFloat) -> Void)?
        override func accessibilityIncrement() { adjust?(12) }
        override func accessibilityDecrement() { adjust?(-12) }
        override func draw(_ rect: CGRect) {
            let corner = tag.nonzeroBitCount == 2
            let size = corner ? CGSize(width: 14, height: 14)
                : tag & 3 != 0 ? CGSize(width: 8, height: 26) : CGSize(width: 26, height: 8)
            let shape = UIBezierPath(roundedRect: CGRect(x: 22 - size.width / 2, y: 22 - size.height / 2,
                                                       width: size.width, height: size.height), cornerRadius: 4)
            UIColor(OBCTheme.surface).setFill(); shape.fill()
            UIColor(OBCTheme.ink).setStroke(); shape.lineWidth = 2; shape.stroke()
        }
    }
}
#endif
