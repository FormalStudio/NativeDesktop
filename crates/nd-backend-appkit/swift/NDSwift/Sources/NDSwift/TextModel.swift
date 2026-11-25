import Foundation
// Weak, so the package still loads on a macOS older than the framework (26).
@_weakLinked import FoundationModels

/// Whether the system's on-device language model can answer now: Apple Intelligence is on and
/// the model has finished downloading.
@_cdecl("nd_swift_text_model_available")
public func ndSwiftTextModelAvailable() -> Bool {
    guard #available(macOS 26.0, *) else { return false }
    if case .available = SystemLanguageModel.default.availability { return true }
    return false
}

/// Called once with the answer or the error text, on a thread of Swift's choosing. The strings
/// live only for the call.
public typealias NDTextModelDone = @convention(c) (
    UnsafeMutableRawPointer?, UnsafePointer<CChar>?, UnsafePointer<CChar>?
) -> Void

/// The caller's context pointer and callback, handed across the task boundary. The caller
/// promises both outlive the one call of `done`.
private struct Reply: @unchecked Sendable {
    let context: UnsafeMutableRawPointer?
    let done: NDTextModelDone

    func answer(_ text: String) { text.withCString { done(context, $0, nil) } }
    func fail(_ why: String) { why.withCString { done(context, nil, $0) } }
}

/// One answer from a fresh session with `instructions`, to `prompt`. Returns at once; `done`
/// runs when the model has answered or failed.
@_cdecl("nd_swift_text_model_respond")
public func ndSwiftTextModelRespond(
    _ instructions: UnsafePointer<CChar>, _ prompt: UnsafePointer<CChar>,
    _ context: UnsafeMutableRawPointer?, _ done: NDTextModelDone
) {
    let reply = Reply(context: context, done: done)
    guard #available(macOS 26.0, *) else {
        reply.fail("the system has no on-device model")
        return
    }
    let (instructions, prompt) = (String(cString: instructions), String(cString: prompt))
    Task.detached {
        do {
            let session = LanguageModelSession(instructions: instructions)
            reply.answer(try await session.respond(to: prompt).content)
        } catch {
            reply.fail(String(describing: error))
        }
    }
}
