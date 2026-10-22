import UniformTypeIdentifiers
import SwiftUI

// MARK: - Compress

struct CompressView: View {
    @Bindable var model: CompressModel
    private let tool = Tool.compress

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "Küçültülecek PDF'leri buraya sürükleyin",
                       subtitle: "Birden fazla PDF seçebilirsiniz · Orijinal dosyalar değişmez")

            VStack(alignment: .leading, spacing: 14) {
                SectionTitle(text: "Sıkıştırma seviyesi")
                HStack(spacing: 12) {
                    ForEach(CompressionLevel.allCases) { level in
                        ChoiceCard(title: level.title, subtitle: level.subtitle, selected: model.level == level, tint: tool.tint) {
                            model.level = level
                        }
                    }
                }
                .fixedSize(horizontal: false, vertical: true)

                Toggle(isOn: $model.scanMode) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Taranmış belge modu").fontWeight(.medium)
                        Text("Her sayfayı görüntüye çevirerek tarama ve fotoğraf içeren PDF'lerde çok daha fazla küçültür. Metin seçilemez hale gelir.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                .toggleStyle(.switch)
                .tint(tool.tint)
                .padding(.top, 4)

                if model.level == .balanced && !model.scanMode {
                    Banner(text: "Makul seçenek dosyayı bozmadan küçültmeyi hedefler. Zaten optimize edilmiş PDF'lerde boyut her zaman azalmayabilir; bu durumda yeni dosya oluşturulmaz.")
                } else {
                    Banner(text: "Sıkıştırmayı artırmak görüntü kalitesini düşürebilir. Sonucu kontrol etmeden orijinal dosyayı silmeyin.",
                           symbol: "exclamationmark.triangle.fill", color: .orange)
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "PDF Küçült",
                      info: model.store.files.isEmpty ? "PDF ekleyin" : "\(model.store.files.count) PDF · \(formatBytes(model.store.totalSize))",
                      disabled: model.store.files.isEmpty, action: model.start)
        }
        .animation(.snappy, value: model.store.files.count)
        .animation(.snappy, value: model.job.results.count)
    }
}

// MARK: - Images → PDF

struct ImagesView: View {
    @Bindable var model: ImagesModel
    private let tool = Tool.imagesToPDF

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "Resimleri buraya sürükleyin",
                       subtitle: "JPG, PNG, WEBP, HEIC, TIFF, GIF, BMP · Sırayı sürükleyerek değiştirin",
                       listTitle: "Sayfa sırası", reorderable: true)

            VStack(alignment: .leading, spacing: 16) {
                SectionTitle(text: "Sayfa ayarları")
                OptionRow(label: "Sayfa boyutu") {
                    Picker("", selection: $model.options.pageSize) {
                        ForEach(PageSize.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)
                }
                OptionRow(label: "Yönlendirme") {
                    Picker("", selection: $model.options.orientation) {
                        ForEach(PageOrientation.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 300)
                    .disabled(model.options.pageSize == .fit)
                }
                OptionRow(label: "Kenar boşluğu") {
                    Picker("", selection: $model.options.margin) {
                        ForEach(PageMargin.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 300)
                }
                OptionRow(label: "Görüntü kalitesi") {
                    Picker("", selection: $model.options.quality) {
                        ForEach(ImageQuality.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)
                }
                OptionRow(label: "Dosya adı") {
                    TextField(model.store.files.first.map { Output.baseName($0.url) } ?? "Belge", text: $model.outputName)
                        .textFieldStyle(.roundedBorder).frame(maxWidth: 300)
                    Text(".pdf").foregroundStyle(.secondary)
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "PDF Oluştur",
                      info: model.store.files.isEmpty ? "Resim ekleyin" : "\(model.store.totalPages) sayfa oluşturulacak",
                      disabled: model.store.files.isEmpty, action: model.start)
        }
        .animation(.snappy, value: model.store.files.count)
    }
}

// MARK: - Merge

struct MergeView: View {
    @Bindable var model: MergeModel
    private let tool = Tool.merge

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "Birleştirilecek PDF'leri sürükleyin",
                       subtitle: "En az iki PDF · Sırayı sürükleyerek değiştirin",
                       listTitle: "Birleştirme sırası", reorderable: true)

            VStack(alignment: .leading, spacing: 14) {
                SectionTitle(text: "Çıktı")
                OptionRow(label: "Dosya adı") {
                    TextField("Birleştirilmiş belge", text: $model.outputName)
                        .textFieldStyle(.roundedBorder).frame(maxWidth: 300)
                    Text(".pdf").foregroundStyle(.secondary)
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "Birleştir",
                      info: model.store.files.count < 2 ? "En az iki PDF ekleyin" : "\(model.store.files.count) dosya · \(model.store.totalPages) sayfa",
                      disabled: model.store.files.count < 2, action: model.start)
        }
        .animation(.snappy, value: model.store.files.count)
    }
}

// MARK: - Split

struct SplitView: View {
    @Bindable var model: SplitModel
    private let tool = Tool.split

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "Bölünecek PDF'i sürükleyin",
                       subtitle: "Tek dosya", multiple: false)

            VStack(alignment: .leading, spacing: 16) {
                SectionTitle(text: "Bölme yöntemi")
                Picker("", selection: $model.mode) {
                    ForEach(SplitModel.Mode.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)

                switch model.mode {
                case .eachPage:
                    Text("Her sayfa ayrı bir PDF dosyası olarak yeni bir klasöre kaydedilir.")
                        .foregroundStyle(.secondary)
                case .ranges:
                    OptionRow(label: "Sayfalar") {
                        TextField("Örn. 1-3, 5, 8-10", text: $model.ranges)
                            .textFieldStyle(.roundedBorder).frame(maxWidth: 300)
                    }
                    Toggle("Her aralığı ayrı dosya yap", isOn: $model.separateRanges)
                        .toggleStyle(.switch).tint(tool.tint)
                    Text(model.separateRanges ? "Virgülle ayrılan her aralık ayrı PDF olur." : "Seçilen tüm sayfalar tek bir PDF'te toplanır.")
                        .font(.caption).foregroundStyle(.secondary)
                case .every:
                    OptionRow(label: "Sayfa sayısı") {
                        Stepper(value: $model.every, in: 1...999) {
                            Text("Her \(model.every) sayfada bir yeni dosya").monospacedDigit()
                        }
                    }
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "Böl",
                      info: model.store.files.first.map { "\($0.pageCount) sayfa" } ?? "PDF ekleyin",
                      disabled: model.store.files.isEmpty || (model.mode == .ranges && model.ranges.trimmingCharacters(in: .whitespaces).isEmpty),
                      action: model.start)
        }
    }
}

// MARK: - PDF → images

struct PDFToImagesView: View {
    @Bindable var model: PDFToImagesModel
    private let tool = Tool.pdfToImages

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "PDF'leri buraya sürükleyin",
                       subtitle: "Her PDF için ayrı bir görsel klasörü oluşturulur")

            VStack(alignment: .leading, spacing: 16) {
                SectionTitle(text: "Görsel ayarları")
                OptionRow(label: "Biçim") {
                    Picker("", selection: $model.format) {
                        ForEach(PDFEngine.ImageFormat.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 200)
                }
                OptionRow(label: "Çözünürlük") {
                    Picker("", selection: $model.dpi) {
                        Text("72 DPI · Ekran").tag(72.0)
                        Text("150 DPI · Standart").tag(150.0)
                        Text("300 DPI · Baskı").tag(300.0)
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "Görsellere Dönüştür",
                      info: model.store.files.isEmpty ? "PDF ekleyin" : "\(model.store.totalPages) sayfa dışa aktarılacak",
                      disabled: model.store.files.isEmpty, action: model.start)
        }
        .animation(.snappy, value: model.store.files.count)
    }
}

// MARK: - Protect

struct ProtectView: View {
    @Bindable var model: ProtectModel
    private let tool = Tool.protect

    var body: some View {
        ToolPage(tool: tool) {
            FilePicker(store: model.store, tool: tool, title: "PDF'leri buraya sürükleyin",
                       subtitle: "Aynı şifre seçilen tüm dosyalara uygulanır")

            VStack(alignment: .leading, spacing: 16) {
                Picker("", selection: $model.mode) {
                    ForEach(ProtectModel.Mode.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 280)

                if model.mode == .encrypt {
                    OptionRow(label: "Açılış şifresi") {
                        SecureField("Şifre", text: $model.password).textFieldStyle(.roundedBorder).frame(maxWidth: 260)
                    }
                    OptionRow(label: "Şifre (tekrar)") {
                        SecureField("Şifreyi tekrar girin", text: $model.confirm).textFieldStyle(.roundedBorder).frame(maxWidth: 260)
                        if !model.confirm.isEmpty && model.confirm != model.password {
                            Label("Eşleşmiyor", systemImage: "xmark.circle.fill").foregroundStyle(.red).font(.caption)
                        }
                    }
                    OptionRow(label: "Sahip şifresi") {
                        SecureField("İsteğe bağlı", text: $model.ownerPassword).textFieldStyle(.roundedBorder).frame(maxWidth: 260)
                    }
                    Banner(text: "Şifreyi unutursanız dosya açılamaz; şifre hiçbir yerde saklanmaz. Orijinal dosyanız şifresiz olarak yerinde kalır.",
                           symbol: "key.fill", color: .orange)
                } else {
                    OptionRow(label: "Mevcut şifre") {
                        SecureField("PDF'in şifresi", text: $model.password).textFieldStyle(.roundedBorder).frame(maxWidth: 260)
                    }
                    Text("Şifresini bildiğiniz PDF'lerin şifresiz bir kopyası oluşturulur.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            .card()

            ResultsView(job: model.job, tint: tool.tint)
        } bar: {
            ActionBar(job: model.job, tool: tool, title: model.mode == .encrypt ? "Şifrele" : "Şifreyi Kaldır",
                      info: model.store.files.isEmpty ? "PDF ekleyin" : "\(model.store.files.count) PDF",
                      disabled: !model.canStart, action: model.start)
        }
    }
}

// MARK: - Shared picker

/// Big drop zone while empty; file list plus a compact "add more" zone afterwards.
struct FilePicker: View {
    let store: FileStore
    let tool: Tool
    let title: String
    let subtitle: String
    var multiple = true
    var listTitle = "Dosyalar"
    var reorderable = false

    private var types: [UTType] { store.kind == .pdf ? FileTypes.pdf : FileTypes.images }

    var body: some View {
        if store.files.isEmpty {
            DropZone(title: title, subtitle: subtitle + "\nveya seçmek için tıklayın", types: types,
                     multiple: multiple, tint: tool.tint) { store.add($0) }
        } else {
            FileListCard(store: store, tint: tool.tint, reorderable: reorderable, title: listTitle)
            DropZone(title: multiple ? "Daha fazla ekle" : "Başka bir dosya seç",
                     subtitle: "Sürükleyip bırakın veya tıklayın", types: types,
                     multiple: multiple, compact: true, tint: tool.tint) { store.add($0) }
        }
    }
}
