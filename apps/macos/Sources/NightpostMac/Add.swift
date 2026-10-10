// Adding someone, as on other systems: a sheet of paper over a veil, for a
// link (with its QR code), a code to read out, or what they gave us; then
// the safety number when a code was used.

import AppKit
import CoreImage.CIFilterBuiltins
import SwiftUI

/// Over the whole window while a dialog is open.
struct DialogLayer: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let dialog = model.dialog {
            ZStack {
                Palette.veil
                    .onTapGesture { if dismissable(dialog) { model.dialog = nil } }
                VStack(alignment: .leading, spacing: 0) {
                    switch dialog {
                    case .add: AddSheet()
                    case let .invite(name, link): InviteSheet(name: name, link: link)
                    case let .code(name, code): CodeSheet(name: name, code: code)
                    case let .confirm(name, number): ConfirmSheet(name: name, number: number)
                    case let .waiting(name, number): WaitingSheet(name: name, number: number)
                    }
                }
                .padding(.horizontal, 30)
                .padding(.top, 26)
                .padding(.bottom, 24)
                .frame(width: 520, alignment: .leading)
                // Frosted: the blurred window shows through a little.
                .background(Palette.panel.opacity(0.72), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Palette.strong.opacity(0.14)))
                .shadow(color: Palette.shadow, radius: 30, y: 30)
                .padding(16)
            }
            .ignoresSafeArea()
            .transition(.opacity)
            // Escape closes, except where an answer is owed.
            .background {
                if dismissable(dialog) {
                    Button("") { model.dialog = nil }.keyboardShortcut(.cancelAction).hidden()
                }
            }
        }
    }

    private func dismissable(_ dialog: Dialog) -> Bool {
        if case .confirm = dialog { return false }
        return true
    }
}

private struct AddSheet: View {
    @Environment(AppModel.self) private var model
    @State private var name = ""
    @State private var given = ""
    @FocusState private var naming: Bool
    @FocusState private var giving: Bool

    var body: some View {
        if let who = model.looking {
            looking(who)
        } else {
            form
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading("Add someone")
            FieldLabel("Their name, as you will see it")
            TextField("", text: $name, prompt: Text("Alice").foregroundStyle(Palette.muted))
                .sheetField($naming)
                .onChange(of: name) { model.addError = "" }
            HStack(spacing: 10) {
                Button("Create a link") { model.invite(trimmed(name), byCode: false) }
                    .buttonStyle(PrimaryButton(height: 40, radius: 8))
                Button("Create a code to read out") { model.invite(trimmed(name), byCode: true) }
                    .buttonStyle(SecondaryButton())
            }
            .padding(.top, 18)
            Rectangle().fill(Palette.line).frame(height: 1).padding(.top, 24).padding(.bottom, 4)
            FieldLabel("Or use what they gave you: their link, or the code they read you")
            TextField("", text: $given, prompt: Text("nightpost:1:… or 482-109-375-512").foregroundStyle(Palette.muted))
                .sheetField($giving)
                .onChange(of: given) { model.addError = "" }
                .onSubmit { model.accept(trimmed(given), name: trimmed(name)) }
            if !model.addError.isEmpty {
                Text(verbatim: model.addError)
                    .font(.ui(15, .semibold))
                    .foregroundStyle(Palette.fail)
                    .padding(.top, 10)
                    .accessibilityAddTraits(.updatesFrequently)
            }
            HStack(spacing: 10) {
                Button("Use it") { model.accept(trimmed(given), name: trimmed(name)) }
                    .buttonStyle(PrimaryButton(height: 40, radius: 8))
                Button("Cancel") { model.dialog = nil }
                    .buttonStyle(SecondaryButton())
            }
            .padding(.top, 18)
        }
        .onAppear {
            model.addError = ""
            // Once the field is in the window; set at once, it is lost.
            Task { naming = true }
        }
    }

    private func looking(_ who: String) -> some View {
        let network = model.network
        let progress = network.percent < 100
            ? "Waiting for Tor (\(network.percent)%) before looking."
            : "Tor is connected. \(who)’s Nightpost has to be running; finding it can take up to 5 minutes."
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Heading("Looking for \(who)…")
                Spacer()
                ProgressView().controlSize(.small).padding(.bottom, 10)
            }
            Paragraph(progress)
            Text("Closing this does not stop it: \(who) shows up in your list once found.")
                .font(.ui(13.5))
                .lineSpacing(3)
                .foregroundStyle(Palette.soft)
            HStack {
                Button("Close") { model.dialog = nil }.buttonStyle(SecondaryButton())
            }
            .padding(.top, 18)
        }
    }

    private func trimmed(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

private struct InviteSheet: View {
    @Environment(AppModel.self) private var model
    let name: String
    let link: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading("An invitation for \(name)")
            Paragraph("Give it to \(name) only, through a channel you trust. It works once, for 7 days.")
            if let qr = QR.image(link) {
                Image(nsImage: qr)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 232, height: 232)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .frame(maxWidth: .infinity)
                    .padding(.top, 6)
                    .padding(.bottom, 16)
                    .accessibilityLabel("QR code of the invitation")
            }
            Text(verbatim: link)
                .font(.ui(12.5))
                .foregroundStyle(Palette.text)
                .textSelection(.enabled)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line2))
            HStack(spacing: 10) {
                CopyButton(text: link, label: "Copy the link")
                Button("Done") { model.dialog = nil }.buttonStyle(SecondaryButton())
            }
            .padding(.top, 18)
        }
    }
}

private struct CodeSheet: View {
    @Environment(AppModel.self) private var model
    let name: String
    let code: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading("Read this code to \(name)")
            Paragraph("On a call or in person. It works for 10 minutes.")
            Text(verbatim: code.replacingOccurrences(of: "-", with: " "))
                .font(.ui(46, .semibold))
                .monospacedDigit()
                .tracking(2)
                .foregroundStyle(Palette.strong)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 22)
            Paragraph("When \(name) types it in, you will both compare a safety number before anything is kept.")
            HStack(spacing: 10) {
                CopyButton(text: code, label: "Copy the code")
                Button("Hide") { model.dialog = nil }.buttonStyle(SecondaryButton())
            }
            .padding(.top, 10)
        }
    }
}

private struct ConfirmSheet: View {
    @Environment(AppModel.self) private var model
    let name: String
    let number: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading("Someone used your code for \(name)")
            Paragraph("Ask them to read their safety number aloud. Yours is:")
            SafetyDigits(number: number)
            Text("Keep them only if every digit matches. Otherwise it is not \(name).")
                .font(.ui(15, .semibold))
                .foregroundStyle(Palette.fail)
            HStack(spacing: 10) {
                Button("The digits match") { model.confirm(name, keep: true) }
                    .buttonStyle(PrimaryButton(height: 40, radius: 8))
                Button("They differ, turn down") { model.confirm(name, keep: false) }
                    .buttonStyle(DangerButton())
            }
            .padding(.top, 18)
        }
    }
}

private struct WaitingSheet: View {
    let name: String
    let number: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading("Waiting for \(name)")
            Paragraph("Read this safety number to \(name). They check that it matches theirs:")
            SafetyDigits(number: number)
            Paragraph("\(name) is added as soon as they confirm.")
        }
    }
}

/// 60 digits as 3 lines of 4 groups, the way they are read out.
struct SafetyDigits: View {
    let number: String
    var size: CGFloat = 22

    var body: some View {
        let digits = Array(number)
        let groups = stride(from: 0, to: digits.count, by: 5).map { String(digits[$0..<min($0 + 5, digits.count)]) }
        let lines = stride(from: 0, to: groups.count, by: 4).map { groups[$0..<min($0 + 4, groups.count)].joined(separator: "   ") }
        Text(verbatim: lines.joined(separator: "\n"))
            .font(.ui(size, .medium))
            .monospacedDigit()
            .lineSpacing(size * 0.5)
            .multilineTextAlignment(.center)
            .foregroundStyle(Palette.strong)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
    }
}

private struct Heading: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(verbatim: text)
            .font(.ui(25, .semibold))
            .foregroundStyle(Palette.strong)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.bottom, 10)
    }
}

private struct Paragraph: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(verbatim: text)
            .font(.ui(15))
            .lineSpacing(4)
            .foregroundStyle(Palette.soft)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.bottom, 8)
    }
}

private struct FieldLabel: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(verbatim: text)
            .font(.ui(13.5))
            .foregroundStyle(Palette.soft)
            .padding(.top, 16)
            .padding(.bottom, 6)
    }
}

private extension View {
    /// style.css's `.sheet input`.
    func sheetField(_ focus: FocusState<Bool>.Binding) -> some View {
        textFieldStyle(.plain)
            .font(.ui(15))
            .foregroundStyle(Palette.text)
            .focused(focus)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(Palette.input, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line2))
            .clickToFocus(focus)
    }
}

/// style.css's `.sheet button`: a line round it.
struct SecondaryButton: ButtonStyle {
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(15))
            .foregroundStyle(Palette.text)
            .padding(.horizontal, 16)
            .frame(height: 40)
            .background(hover || configuration.isPressed ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line2))
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}

struct DangerButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(15, .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
            .frame(height: 40)
            .background(Palette.fail, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .brightness(configuration.isPressed ? -0.05 : 0)
            .contentShape(Rectangle())
    }
}

/// Copies `text`, and says so for four seconds; both labels hold the width.
struct CopyButton: View {
    let text: String
    let label: String
    @State private var copied = false

    var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            copied = NSPasteboard.general.setString(text, forType: .string)
            Task {
                try? await Task.sleep(for: .seconds(4))
                copied = false
            }
        } label: {
            ZStack {
                Text(label).opacity(copied ? 0 : 1)
                Text("Copied").opacity(copied ? 1 : 0)
            }
        }
        .buttonStyle(PrimaryButton(height: 40, radius: 8))
        .accessibilityLabel(copied ? "Copied" : label)
    }
}

enum QR {
    /// Ink on paper: not every phone reads it inverted. Upper case fits the
    /// link in QR's denser alphanumeric mode, as on other systems.
    static func image(_ link: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(link.uppercased().utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let colored = output.applyingFilter("CIFalseColor", parameters: [
            "inputColor0": CIColor(red: 0x10 / 255, green: 0x14 / 255, blue: 0x1D / 255),
            "inputColor1": CIColor(red: 1, green: 1, blue: 1),
        ])
        // A quiet zone of 2 modules, then one pixel per module: the view scales it up.
        let framed = colored.transformed(by: CGAffineTransform(translationX: 2, y: 2))
            .composited(over: CIImage(color: .white).cropped(to: output.extent.insetBy(dx: -2, dy: -2).offsetBy(dx: 2, dy: 2)))
        guard let cg = CIContext().createCGImage(framed, from: framed.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}
