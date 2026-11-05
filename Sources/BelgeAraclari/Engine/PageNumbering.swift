import Foundation

/// "Sayfa numarası ekle" on the screens that make PDFs. The numbers are stamped by the shared core
/// ([BelgeCore]) onto the finished file, so they look the same as on Windows.
struct PageNumbering: Equatable, Sendable {
    enum Position: String, CaseIterable, Sendable {
        case topLeft = "top-left", topCenter = "top-center", topRight = "top-right"
        case bottomLeft = "bottom-left", bottomCenter = "bottom-center", bottomRight = "bottom-right"

        var title: String {
            switch self {
            case .topLeft: return "Sol üst"
            case .topCenter: return "Orta üst"
            case .topRight: return "Sağ üst"
            case .bottomLeft: return "Sol alt"
            case .bottomCenter: return "Orta alt"
            case .bottomRight: return "Sağ alt"
            }
        }
    }

    enum Format: String, CaseIterable, Sendable {
        case number = "n", ofTotal = "n/N", page = "sayfa-n", pageOfTotal = "sayfa-n/N", dashes = "dash"

        var title: String {
            switch self {
            case .number: return "1"
            case .ofTotal: return "1 / 12"
            case .page: return "Sayfa 1"
            case .pageOfTotal: return "Sayfa 1 / 12"
            case .dashes: return "- 1 -"
            }
        }
    }

    var enabled = false
    var position = Position.bottomRight
    var format = Format.pageOfTotal

    func stamp(_ url: URL) throws {
        let result = try BelgeCore.run(["number-pages", url.path, position.rawValue, format.rawValue])
        guard result.status == 0 else {
            throw AppError.message(result.stderr.isEmpty ? "\"\(url.lastPathComponent)\" için sayfa numarası eklenemedi." : result.stderr)
        }
    }

    /// Numbers every PDF the job made. A failure is reported on that file's line.
    func apply(_ results: [ResultItem], skip originals: Set<URL> = []) -> [ResultItem] {
        guard enabled else { return results }
        return results.map { item in
            guard item.success, let url = item.url, FileTypes.isPDF(url), !originals.contains(url) else { return item }
            var item = item
            do {
                try stamp(url)
                item.detail += " · sayfa numaralı"
            } catch {
                item.detail += " · \(error.localizedDescription)"
            }
            return item
        }
    }
}
