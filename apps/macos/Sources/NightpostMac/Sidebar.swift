import SwiftUI

struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let people = model.sortedContacts
        let reachable = people.filter { Route.reached(model.network, model.links[$0.name]) }.count
        List(selection: Binding(get: { model.selected }, set: { model.select($0) })) {
            Section {
                ForEach(people) { contact in
                    ContactRow(contact: contact).tag(contact.name)
                }
            } header: {
                if !people.isEmpty { Text("\(reachable) reachable of \(people.count)") }
            }
        }
        .navigationSplitViewColumnWidth(min: 240, ideal: 290)
        .safeAreaInset(edge: .bottom) {
            let (stage, title) = networkSummary(model.network)
            HStack(spacing: 8) {
                Circle().fill(stage == .done ? Color.secondary : stage.color).frame(width: 8, height: 8)
                Text(title).foregroundStyle(stage == .fail ? Palette.fail : .secondary)
            }
            .font(.callout)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
    }
}

struct ContactRow: View {
    @Environment(AppModel.self) private var model
    let contact: ContactInfo

    var body: some View {
        let last = model.messages[contact.name]?.last
        let unread = model.unread[contact.name] ?? 0
        let on = Route.reached(model.network, model.links[contact.name])
        HStack(spacing: 10) {
            Avatar(name: contact.name, photo: contact.person.photo, size: 38)
                .overlay(alignment: .bottomTrailing) {
                    if on {
                        Circle().fill(Palette.accent).frame(width: 11, height: 11)
                            .overlay(Circle().stroke(.background, lineWidth: 2))
                    }
                }
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline) {
                    Text(contact.name).fontWeight(unread > 0 ? .bold : .medium).lineLimit(1)
                    Spacer(minLength: 4)
                    if let last { Text(When.short(last.time)).font(.caption).foregroundStyle(.secondary) }
                }
                HStack {
                    Text(verbatim: preview(last))
                        .lineLimit(1)
                        .foregroundStyle(unread > 0 ? .primary : .secondary)
                    Spacer(minLength: 4)
                    if unread > 0 {
                        Text("\(unread)")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(Palette.accentInk)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(Palette.accent, in: Capsule())
                    }
                }
                .font(.callout)
            }
        }
        .padding(.vertical, 4)
    }

    private func preview(_ last: HistoryItem?) -> String {
        guard let last else { return "No messages yet" }
        let line = last.text.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
        return last.outgoing ? "You: \(line)" : line
    }
}
