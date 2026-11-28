import ObjectiveC
import UIKit

/// The system's own three-finger editing gestures (its undo and redo swipes, the tap that shows its
/// shortcut bar, the copy and paste pinches) stay off everywhere but in text being typed: three
/// fingers on the canvas are the app's (tapped they redo, swiped down they open Copy & Paste,
/// scrubbed they clear the layer), and the system's would take the touches first.
///
/// The system asks the first responder whether it wants them, as the first responder changes.
/// The canvas's controller and view say no, but the views and controllers SwiftUI makes itself
/// (the app's root among them) take that place at times, and the app can't subclass them: so
/// every responder says no unless it is text being typed into, which keeps the system's.
enum EditingGestures {
    private static var turnedOff = false

    /// The answer every responder gives: none, but for text being typed into.
    private static let answer: IMP = {
        let own: @convention(block) (UIResponder) -> UIEditingInteractionConfiguration = { responder in
            responder is UITextInput ? .default : .none
        }
        return imp_implementationWithBlock(own)
    }()

    private static let asked = #selector(getter: UIResponder.editingInteractionConfiguration)

    /// Once, as the app starts, before any responder is asked: the system's kinds of responder
    /// answer so (any of them that answers for itself rather than as a responder does), and each
    /// kind under them alike unless it answers for itself.
    static func turnOff() {
        guard !turnedOff else { return }
        turnedOff = true
        var replaced = 0
        for kind: AnyClass in [UIResponder.self, UIView.self, UIViewController.self, UIWindow.self] {
            if replaceOwn(kind) {
                replaced += 1
            }
        }
        if replaced == 0 {
            AppLog.error("input", "the system's editing gestures couldn't be turned off")
        }
    }

    /// Every responder from `responder` out to the app (SwiftUI's own views and controllers
    /// round the canvas, its window) answers as the rest do, should any kind of them answer for
    /// itself.
    static func cover(from responder: UIResponder) {
        turnOff()
        var next: UIResponder? = responder
        while let at = next {
            var kind: AnyClass? = type(of: at)
            while let k = kind, ObjectIdentifier(k) != ObjectIdentifier(UIResponder.self) {
                _ = replaceOwn(k)
                kind = class_getSuperclass(k)
            }
            next = at.next
        }
    }

    /// `kind`'s own answer, if it has one (not one it takes from the kind above it), replaced.
    /// Returns whether it was.
    private static func replaceOwn(_ kind: AnyClass) -> Bool {
        guard let method = class_getInstanceMethod(kind, asked) else { return false }
        let imp = method_getImplementation(method)
        if imp == answer {
            return false
        }
        if let above = class_getSuperclass(kind), let inherited = class_getInstanceMethod(above, asked),
           method_getImplementation(inherited) == imp
        {
            return false
        }
        method_setImplementation(method, answer)
        return true
    }
}
