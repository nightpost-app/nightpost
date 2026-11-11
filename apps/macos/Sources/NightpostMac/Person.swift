// Someone's panel, beside the conversation, as on other systems: photo,
// name, safety number, notifications, removal.

import AppKit
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

private let hour: UInt64 = 3_600_000

private func now() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }

struct PersonPanel: View {
    @Environment(AppModel.self) private var model
    let name: String
    @State private var newName = ""
    @State private var removing = false
    @State private var wipe = false
    @FocusState private var naming: Bool

    var body: some View {
        let person = model.contacts.first { $0.name == name }?.person ?? Person()
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                photo(person)
                renaming
                safety(person)
                notifications(person)
                timer
                removal
            }
            .padding(.horizontal, 22)
            .padding(.top, 62)
            .padding(.bottom, 28)
        }
        .scrollIndicators(.never)
        // The close button stays put while the panel scrolls under it.
        .overlay(alignment: .topLeading) {
            Button { model.panel = false } label: {
                Icon(parts: [.path("M6 6l12 12M18 6L6 18")], weight: 2).frame(width: 18, height: 18)
            }
            .buttonStyle(QuietButton())
            .help(t("close"))
            .padding(.leading, 12)
            .padding(.top, 14)
        }
        .frame(maxWidth: 340, maxHeight: .infinity)
        .background(Palette.side)
        .overlay(alignment: .leading) { Rectangle().fill(Palette.line).frame(width: 1) }
        .shadow(color: Palette.shadow, radius: 20, x: -18)
        .onAppear { newName = name }
    }

    private func photo(_ person: Person) -> some View {
        VStack(spacing: 12) {
            Avatar(name: name, photo: person.photo, size: 96)
            HStack(spacing: 8) {
                Button(person.photo == nil ? t("choose-photo") : t("change-photo"), action: choosePhoto)
                    .buttonStyle(SmallButton())
                if person.photo != nil {
                    Button(t("use-rings")) { model.update(name) { $0.photo = nil } }
                        .buttonStyle(SmallButton())
                }
            }
            Help(t("photo-help", ["name": name]))
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
    }

    private var renaming: some View {
        let to = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        return VStack(alignment: .leading, spacing: 12) {
            PanelTitle(t("name"))
            HStack(spacing: 8) {
                TextField(t("name"), text: $newName)
                    .textFieldStyle(.plain)
                    .font(.ui(15))
                    .foregroundStyle(Palette.strong)
                    .focused($naming)
                    .padding(.horizontal, 12)
                    .frame(height: 38)
                    .background(Palette.input, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Palette.line2))
                    .clickToFocus($naming)
                    .onChange(of: newName) { if newName.count > 32 { newName = String(newName.prefix(32)) } }
                    .onSubmit { if !to.isEmpty && to != name { model.rename(name, to: to) } }
                Button(t("rename")) { model.rename(name, to: to) }
                    .buttonStyle(SmallButton())
                    .disabled(to.isEmpty || to == name)
            }
            Help(t("name-help"))
        }
    }

    private func safety(_ person: Person) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                PanelTitle(t("safety-number"))
                Spacer()
                Text(person.verified ? t("verified") : t("not-compared"))
                    .font(.ui(12.5, .semibold))
                    .foregroundStyle(person.verified ? Palette.main : Palette.muted)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 3)
                    .background(person.verified ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(person.verified ? Palette.main : Palette.line2))
            }
            Help(person.verified
                ? t("compared-help", ["name": name])
                : t("compare-help", ["name": name]))
            Button(t("compare")) { model.compare(name) }
                .buttonStyle(SmallButton())
        }
    }

    private func notifications(_ person: Person) -> some View {
        let quiet = person.mutedUntil.flatMap { $0 > now() ? $0 : nil }
        let choice: UInt64? = switch quiet {
        case nil: nil
        case UInt64.max: .max
        case let until?: until - now() > 2 * hour ? 8 * hour : hour
        }
        let help = switch quiet {
        case nil: t("mute-on-help", ["name": name])
        case UInt64.max: t("mute-off-help", ["name": name])
        case let until?: t("mute-until-help", ["time": When.clock(until)])
        }
        let choices: [(String, UInt64?)] = [(t("mute-on"), nil), (t("mute-1h"), hour), (t("mute-8h"), 8 * hour), (t("mute-off"), .max)]
        return VStack(alignment: .leading, spacing: 12) {
            PanelTitle(t("notifications"))
            HStack(spacing: 4) {
                ForEach(choices, id: \.0) { label, length in
                    Button(label) {
                        let until = length.map { $0 == .max ? $0 : now() + $0 }
                        model.update(name) { $0.mutedUntil = until }
                    }
                    .buttonStyle(Segment(on: length == choice))
                }
            }
            .padding(4)
            .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
            Help(help)
        }
    }

    private var timer: some View {
        let seconds = model.contacts.first { $0.name == name }?.timer ?? 0
        return VStack(alignment: .leading, spacing: 12) {
            PanelTitle(t("timer-title"))
            Picker(t("timer-title"), selection: Binding(get: { seconds }, set: { model.setTimer(name, $0) })) {
                ForEach(timerChoices, id: \.self) { Text(timerLabel($0)).tag($0) }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .fixedSize()
            Help(t("timer-help", ["name": name]))
        }
    }

    private var removal: some View {
        VStack(alignment: .leading, spacing: 12) {
            if removing {
                PanelTitle(t("remove-title", ["name": name]), color: Palette.fail)
                Help(t("remove-help", ["name": name]))
                Button { wipe.toggle() } label: {
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 5, style: .continuous)
                            .fill(wipe ? Palette.fail : .clear)
                            .strokeBorder(wipe ? Palette.fail : Palette.muted, lineWidth: 2)
                            .frame(width: 18, height: 18)
                            .overlay { if wipe { Text("✓").font(.system(size: 13, weight: .bold)).foregroundStyle(.white) } }
                        Text(t("remove-wipe"))
                            .font(.ui(14))
                            .foregroundStyle(Palette.text)
                            .multilineTextAlignment(.leading)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(wipe ? [.isSelected] : [])
                HStack(spacing: 8) {
                    Button(t("remove")) { model.remove(name, wipe: wipe) }.buttonStyle(DangerButton())
                    Button(t("cancel")) {
                        removing = false
                        wipe = false
                    }
                    .buttonStyle(SmallButton())
                }
            } else {
                Button(t("remove-name", ["name": name])) { removing = true }.buttonStyle(SmallButton(color: Palette.fail))
            }
        }
        .padding(.top, 18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .top) { Rectangle().fill(Palette.line).frame(height: 1) }
    }

    private func choosePhoto() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .webP, .gif]
        panel.begin { answer in
            guard answer == .OK, let url = panel.url else { return }
            do {
                let uri = try Photo.uri(url)
                model.update(name) { $0.photo = uri }
            } catch {
                model.status = t("photo-failed", ["error": error.localizedDescription])
            }
        }
    }
}

/// The digits to read out, over the blurred window.
struct SafetyScreen: View {
    @Environment(AppModel.self) private var model
    let name: String
    let number: String

    var body: some View {
        let photo = model.contacts.first { $0.name == name }?.person.photo
        VStack(spacing: 20) {
            Avatar(name: name, photo: photo, size: 64)
            Text(t("safety-title", ["name": name]))
                .font(.ui(30, .bold))
                .foregroundStyle(Palette.strong)
                .multilineTextAlignment(.center)
            Text(t("safety-help"))
                .font(.ui(16))
                .lineSpacing(4)
                .foregroundStyle(Palette.soft)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 460)
            SafetyDigits(number: number, size: 34)
                .fixedSize()
                .padding(.horizontal, 34)
                .padding(.vertical, 6)
                .background(Palette.tip.opacity(0.55), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Palette.strong.opacity(0.14)))
                .shadow(color: Palette.shadow, radius: 30, y: 24)
                .padding(.vertical, 6)
            HStack(spacing: 10) {
                Button(t("they-match")) {
                    model.dialog = nil
                    model.update(name) { $0.verified = true }
                }
                .buttonStyle(PrimaryButton(height: 48, radius: 12))
                Button(t("not-now")) { model.dialog = nil }.buttonStyle(SecondaryButton(height: 48, radius: 12))
            }
            Text(t("safety-warning", ["name": name]))
                .font(.ui(14, .medium))
                .foregroundStyle(Palette.fail)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: 620)
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // Nearly opaque: the digits must read whatever is behind.
        .background(Palette.ground.opacity(0.85))
        .overlay(alignment: .topTrailing) {
            Button { model.dialog = nil } label: {
                Icon(parts: [.path("M6 6l12 12M18 6L6 18")], weight: 2).frame(width: 22, height: 22)
            }
            .buttonStyle(OutlineButton())
            .help(t("close"))
            .padding(18)
        }
    }
}

/// The picked image, decoded and re-encoded as a 256 × 256 PNG: whatever
/// the file held besides pixels does not survive.
enum Photo {
    /// The daemon's MAX_PHOTO.
    static let limit = 512 * 1024

    struct Unusable: LocalizedError {
        let errorDescription: String?
    }

    static func uri(_ url: URL) throws -> String {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        guard size <= 30 << 20 else { throw Unusable(errorDescription: t("photo-too-big")) }
        // A thumbnail: ImageIO never decodes more than this many pixels.
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 1024,
        ] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else {
            throw Unusable(errorDescription: t("mac-photo-not-picture"))
        }
        let side = min(image.width, image.height)
        guard let square = image.cropping(to: CGRect(x: (image.width - side) / 2, y: (image.height - side) / 2, width: side, height: side)),
              let context = CGContext(data: nil, width: 256, height: 256, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw Unusable(errorDescription: t("mac-photo-resize")) }
        context.interpolationQuality = .high
        context.draw(square, in: CGRect(x: 0, y: 0, width: 256, height: 256))
        guard let resized = context.makeImage(),
              let png = NSBitmapImageRep(cgImage: resized).representation(using: .png, properties: [:]) else {
            throw Unusable(errorDescription: t("mac-photo-reencode"))
        }
        let uri = "data:image/png;base64," + png.base64EncodedString()
        guard uri.utf8.count <= limit else { throw Unusable(errorDescription: t("photo-too-detailed")) }
        return uri
    }
}

struct PanelTitle: View {
    let text: String
    var color = Palette.strong
    init(_ text: String, color: Color = Palette.strong) {
        self.text = text
        self.color = color
    }

    var body: some View {
        Text(verbatim: text).font(.ui(16, .semibold)).foregroundStyle(color)
    }
}

struct Help: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(verbatim: text)
            .font(.ui(13.5))
            .lineSpacing(3)
            .foregroundStyle(Palette.soft)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// style.css's `.small-button`.
struct SmallButton: ButtonStyle {
    var color = Palette.text
    @Environment(\.isEnabled) private var enabled
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(14, .medium))
            .foregroundStyle(color)
            .lineLimit(1)
            .padding(.horizontal, 14)
            .frame(height: 38)
            .background(enabled && (hover || configuration.isPressed) ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(color == Palette.text ? Palette.line2 : color))
            .opacity(enabled ? 1 : 0.45)
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}

/// One choice of style.css's `.segmented`.
private struct Segment: ButtonStyle {
    let on: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(13.5, on ? .semibold : .medium))
            .foregroundStyle(on ? Palette.mainInk : Palette.soft)
            .frame(maxWidth: .infinity)
            .frame(height: 34)
            .background(on ? Palette.main : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
            .accessibilityAddTraits(on ? [.isSelected] : [])
    }
}

/// An icon button with no frame until pointed at.
struct QuietButton: ButtonStyle {
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(hover ? Palette.strong : Palette.soft)
            .frame(width: 36, height: 36)
            .background(hover ? Palette.selected : Palette.side, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}
