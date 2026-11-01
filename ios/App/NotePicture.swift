import PhotosUI
import SwiftUI

/// Pictures already loaded, so scrolling back to one does not ask again.
private let pictureCache = NSCache<NSString, UIImage>()

/// A picture in a note: `![alt](attachments/name.jpg)` is read from this phone's pictures folder or
/// fetched from the shared library (and kept on the phone, so it is there after the app is closed
/// and reopened); an `https://` address is downloaded. Tapping it opens it full screen, where it
/// can be saved or shared.
struct NotePicture: View {
    let src: String
    let alt: String
    let width: CGFloat?
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @State private var image: UIImage?
    @State private var failure: String?
    @State private var loading = false
    @State private var viewing = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: width ?? .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .onTapGesture { viewing = true }
                    .accessibilityLabel(alt.isEmpty ? "Picture" : alt)
                    .accessibilityAddTraits(.isButton)
            } else if let failure {
                Button {
                    Task { await load(force: true) }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "photo.badge.exclamationmark")
                        VStack(alignment: .leading, spacing: 2) {
                            Text(alt.isEmpty ? "Picture" : alt).font(AppFont.body(15))
                            Text("\(failure) Tap to try again.").font(AppFont.body(12))
                        }
                    }
                    .foregroundStyle(palette.fgMuted)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(palette.elevated, in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
            } else {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading the picture…").font(AppFont.body(14)).foregroundStyle(palette.fgMuted)
                }
                .padding(10)
            }
        }
        .task(id: src) { await load(force: false) }
        .fullScreenCover(isPresented: $viewing) {
            if let image { PictureViewer(image: image, name: alt.isEmpty ? "picture" : alt) }
        }
    }

    private func load(force: Bool) async {
        if !force, let kept = pictureCache.object(forKey: src as NSString) {
            image = kept
            return
        }
        guard !loading else { return }
        loading = true
        failure = nil
        defer { loading = false }
        do {
            let bytes: Data
            let lower = src.lowercased()
            if lower.hasPrefix("https://") || lower.hasPrefix("http://") {
                guard let url = URL(string: src) else { throw PictureError.unreadable }
                bytes = try await URLSession.shared.data(from: url).0
            } else {
                let wanted = src
                bytes = try await library.run { Data(try $0.image(src: wanted)) }
            }
            guard let decoded = UIImage(data: bytes) else { throw PictureError.unreadable }
            pictureCache.setObject(decoded, forKey: src as NSString)
            image = decoded
        } catch {
            failure = library.describe(error)
        }
    }
}

private enum PictureError: LocalizedError {
    case unreadable
    var errorDescription: String? { "This is not a picture the app can show." }
}

/// A picture full screen: pinch to zoom, save it to Photos, or share it.
struct PictureViewer: View {
    let image: UIImage
    let name: String
    @Environment(\.dismiss) private var dismiss
    @State private var saved = false
    @State private var scale: CGFloat = 1
    @State private var settled: CGFloat = 1

    var body: some View {
        NavigationStack {
            GeometryReader { box in
                ScrollView([.horizontal, .vertical]) {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(width: box.size.width * scale, height: box.size.height * scale)
                }
                .gesture(
                    MagnifyGesture()
                        .onChanged { scale = max(1, min(6, settled * $0.magnification)) }
                        .onEnded { _ in settled = scale })
            }
            .background(Color.black)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Done") { dismiss() } }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        UIImageWriteToSavedPhotosAlbum(image, nil, nil, nil)
                        saved = true
                    } label: {
                        Label(saved ? "Saved" : "Save to Photos", systemImage: saved ? "checkmark" : "square.and.arrow.down")
                    }
                    ShareLink(
                        item: Image(uiImage: image), preview: SharePreview(name, image: Image(uiImage: image)))
                }
            }
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbarColorScheme(.dark, for: .navigationBar)
        }
    }
}

/// What a picture chosen from the photo library turns into before it goes in a note: no side
/// longer than 2048 points, as a JPEG, so a photo from the camera is a few hundred kilobytes
/// rather than ten megabytes (the library only keeps pictures up to 12 MB).
enum PictureExport {
    static func jpeg(from data: Data) -> Data? {
        guard let source = UIImage(data: data) else { return nil }
        let longest = max(source.size.width, source.size.height)
        let factor = longest > 2048 ? 2048 / longest : 1
        let size = CGSize(width: source.size.width * factor, height: source.size.height * factor)
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        format.opaque = true
        let drawn = UIGraphicsImageRenderer(size: size, format: format).image { context in
            UIColor.white.setFill()
            context.fill(CGRect(origin: .zero, size: size))
            source.draw(in: CGRect(origin: .zero, size: size))
        }
        return drawn.jpegData(compressionQuality: 0.85)
    }
}
