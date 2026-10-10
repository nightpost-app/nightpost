import SwiftUI

/// The list of people, as on other systems: the wordmark, everyone with
/// their last message, and Tor's state with the main actions underneath.
struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let people = model.sortedContacts
        let reachable = people.filter { Route.reached(model.network, model.links[$0.name]) }.count
        VStack(spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text("Nightpost").font(.ui(24, .bold)).foregroundStyle(Palette.strong)
                Spacer()
                if !people.isEmpty {
                    Text("\(reachable) reachable of \(people.count)")
                        .font(.ui(13))
                        .monospacedDigit()
                        .foregroundStyle(Palette.muted)
                }
            }
            .padding(.top, Chrome.top + 12)
            .padding(.horizontal, 16)
            .padding(.bottom, 14)
            .contentShape(Rectangle())
            .gesture(WindowDragGesture())
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(people) { contact in
                        ContactRow(contact: contact, selected: model.selected == contact.name) {
                            model.select(contact.name)
                        }
                    }
                }
                .padding(.horizontal, 10)
            }
            .scrollIndicators(.never)
            footer
        }
        .background(Palette.side)
        .overlay(alignment: .trailing) { Rectangle().fill(Palette.line).frame(width: 1) }
    }

    private var footer: some View {
        let (stage, title) = networkSummary(model.network)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Circle().fill(stage.color).frame(width: 8, height: 8)
                Text(title).font(.ui(13.5, .medium)).foregroundStyle(Palette.soft)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            HStack(spacing: 8) {
                Button("Add someone") { model.adding = true }
                    .buttonStyle(PrimaryButton(wide: true))
                SettingsLink {
                    Image(systemName: "gearshape").font(.system(size: 17))
                }
                .buttonStyle(OutlineButton())
                .help("Settings")
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 14)
        .overlay(alignment: .top) { Rectangle().fill(Palette.line).frame(height: 1) }
    }
}

struct ContactRow: View {
    @Environment(AppModel.self) private var model
    let contact: ContactInfo
    let selected: Bool
    let pick: () -> Void
    @State private var hover = false

    var body: some View {
        let last = model.messages[contact.name]?.last
        let unread = model.unread[contact.name] ?? 0
        let on = Route.reached(model.network, model.links[contact.name])
        Button(action: pick) {
            HStack(spacing: 12) {
                Avatar(name: contact.name, photo: contact.person.photo, size: 44)
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .strokeBorder(on ? Palette.side : Palette.muted, lineWidth: 2)
                            .background(Circle().fill(on ? Palette.main : Palette.side))
                            .frame(width: 12, height: 12)
                            .offset(x: 1, y: 1)
                    }
                VStack(alignment: .leading, spacing: 1) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(contact.name)
                            .font(.ui(15.5, unread > 0 ? .bold : .semibold))
                            .foregroundStyle(Palette.strong)
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        if let last {
                            Text(When.short(last.time)).font(.ui(13)).monospacedDigit().foregroundStyle(Palette.muted)
                        }
                    }
                    HStack(spacing: 8) {
                        Text(verbatim: preview(last))
                            .font(.ui(13.5))
                            .foregroundStyle(unread > 0 ? Palette.strong : Palette.soft)
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        if unread > 0 {
                            Text("\(unread)")
                                .font(.ui(12, .bold))
                                .foregroundStyle(Palette.mainInk)
                                .frame(minWidth: 20)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 1)
                                .background(Palette.main, in: Capsule())
                        }
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .background(selected || hover ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }

    private func preview(_ last: HistoryItem?) -> String {
        guard let last else { return "No messages yet" }
        let line = last.text.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
        return last.outgoing ? "You: \(line)" : line
    }
}
