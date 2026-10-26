import Foundation

/// The shared Rust core (`core/`), bundled next to the app executable as `belge-core`. The Windows
/// app links the same code directly.
enum BelgeCore {
    struct Output {
        let status: Int32
        let stdout: String
        let stderr: String
    }

    static func tool() throws -> URL {
        guard let url = Bundle.main.url(forAuxiliaryExecutable: "belge-core") else {
            throw AppError.message("Dönüştürücü bileşeni uygulama paketinde bulunamadı. Uygulamayı yeniden yükleyin.")
        }
        return url
    }

    static func run(_ arguments: [String]) throws -> Output {
        let process = Process()
        process.executableURL = try tool()
        process.arguments = arguments
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        try process.run()
        // Read both pipes concurrently so a chatty child cannot block on a full buffer.
        var errorData = Data()
        let group = DispatchGroup()
        group.enter()
        DispatchQueue.global().async {
            errorData = err.fileHandleForReading.readDataToEndOfFile()
            group.leave()
        }
        let outputData = out.fileHandleForReading.readDataToEndOfFile()
        group.wait()
        process.waitUntilExit()
        let text = { (data: Data) in String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
        return Output(status: process.terminationStatus, stdout: text(outputData), stderr: text(errorData))
    }

    /// Office suites found on this Mac (Microsoft Word / Excel / PowerPoint, LibreOffice).
    static func installedOffice() -> [String] {
        guard let result = try? run(["office-engines"]), result.status == 0 else { return [] }
        return result.stdout.split(separator: "\n").map(String.init)
    }
}
