import SwiftUI

enum BoopmarkTheme {
    static let background = Color(red: 15 / 255, green: 17 / 255, blue: 23 / 255)
    static let surface = Color(red: 30 / 255, green: 34 / 255, blue: 53 / 255)
    static let imagePlaceholder = Color(red: 21 / 255, green: 24 / 255, blue: 39 / 255)
    static let surfaceRaised = Color(red: 42 / 255, green: 45 / 255, blue: 69 / 255)
    static let primary = Color(red: 37 / 255, green: 99 / 255, blue: 235 / 255)
    static let mustard = Color(red: 245 / 255, green: 197 / 255, blue: 66 / 255)
    static let muted = Color(red: 156 / 255, green: 163 / 255, blue: 175 / 255)
}

struct BrandMark: View {
    var size: CGFloat = 34

    var body: some View {
        Image("BoopmarkLogo")
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityLabel("Boopmark")
    }
}

struct TagPill: View {
    let tag: String

    var body: some View {
        Text(tag)
            .font(.caption2.weight(.medium))
            .foregroundStyle(BoopmarkTheme.muted)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(BoopmarkTheme.surfaceRaised, in: Capsule())
            .accessibilityIdentifier("bookmark.tag.\(tag)")
    }
}

extension View {
    func boopmarkCard() -> some View {
        self
            .padding(16)
            .background(BoopmarkTheme.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .stroke(Color.white.opacity(0.06), lineWidth: 1)
            }
    }
}
