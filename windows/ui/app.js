// Belge Araçları — Windows interface. All work happens locally through Tauri commands;
// this file never makes network requests.
import { icon } from './icons.js';
import { closePdf, openPdf, renderPage } from './render.js';

const tauri = () => window.__TAURI__;
const invoke = (cmd, args, options) => {
  const api = tauri();
  if (!api) return Promise.reject(new Error('Bu sayfa yalnızca uygulama içinde çalışır.'));
  return api.core.invoke(cmd, args, options);
};

// MARK: - Tools

const TOOLS = [
  { id: 'compress', icon: 'compress', title: 'PDF Küçült', tint: 'var(--green)', accepts: 'pdf',
    subtitle: 'PDF dosyalarını makul kaliteyle sıkıştırın veya daha küçük hale getirin.' },
  { id: 'images', icon: 'images', title: "Resimden PDF'e", tint: 'var(--blue)', accepts: 'image',
    subtitle: 'JPG, PNG, WEBP ve TIFF resimlerini tek PDF yapın.' },
  { id: 'merge', icon: 'merge', title: 'PDF Birleştir', tint: 'var(--purple)', accepts: 'pdf',
    subtitle: "Birden fazla PDF'i istediğiniz sırayla tek dosyada birleştirin." },
  { id: 'split', icon: 'split', title: 'PDF Böl', tint: 'var(--orange)', accepts: 'pdf', single: true,
    subtitle: 'Her sayfayı ayrı dosya yapın veya belirli sayfaları çıkarın.' },
  { id: 'organize', icon: 'organize', title: 'Sayfaları Düzenle', tint: 'var(--teal)', accepts: 'pdf', single: true,
    subtitle: 'Sayfaları döndürün, silin ve sürükleyerek yeniden sıralayın.' },
  { id: 'pdfToImages', icon: 'pdfToImages', title: "PDF'ten Resme", tint: 'var(--pink)', accepts: 'pdf',
    subtitle: 'PDF sayfalarını yüksek çözünürlüklü PNG veya JPEG olarak kaydedin.' },
  { id: 'protect', icon: 'protect', title: 'PDF Şifrele', tint: 'var(--slate)', accepts: 'pdf',
    subtitle: "PDF'e açılış şifresi ekleyin veya bildiğiniz şifreyi kaldırın." },
];
const toolById = Object.fromEntries(TOOLS.map((t) => [t.id, t]));

const LEVELS = [
  { id: 'balanced', title: 'Makul', text: 'Kalite ve boyut arasında önerilen dengeli seçim.', scanDpi: 150, jpeg: 0.72 },
  { id: 'smaller', title: 'Daha Küçük', text: 'Görsel kaliteyi biraz azaltarak daha fazla küçültür.', scanDpi: 120, jpeg: 0.55 },
  { id: 'smallest', title: 'En Küçük', text: 'En agresif sıkıştırma. Görsel kalite belirgin düşebilir.', scanDpi: 96, jpeg: 0.4 },
];

// MARK: - State

const newJob = () => ({ id: null, running: false, progress: 0, status: '', results: [], error: null });

function loadSettings() {
  try { return { outMode: 'next', outDir: '', useGs: true, ...JSON.parse(localStorage.getItem('settings') || '{}') }; }
  catch { return { outMode: 'next', outDir: '', useGs: true }; }
}

const state = {
  view: 'home',
  settings: loadSettings(),
  tools: {
    compress: { files: [], level: 'balanced', scan: false, job: newJob() },
    images: { files: [], options: { pageSize: 'a4', orientation: 'auto', margin: 'none', quality: 'balanced' }, name: '', job: newJob() },
    merge: { files: [], name: 'Birleştirilmiş belge', job: newJob() },
    split: { files: [], mode: 'each', ranges: '', separate: false, every: 2, job: newJob() },
    organize: { file: null, doc: null, pages: [], selected: new Set(), original: 0, error: null, job: newJob() },
    pdfToImages: { files: [], format: 'png', dpi: 150, job: newJob() },
    protect: { files: [], mode: 'encrypt', password: '', confirm: '', owner: '', job: newJob() },
  },
};
const jobsById = new Map();
const thumbCache = new Map();
const pageThumbs = new Map();

const outDir = () => (state.settings.outMode === 'folder' && state.settings.outDir ? state.settings.outDir : null);
const saveSettings = () => { try { localStorage.setItem('settings', JSON.stringify(state.settings)); } catch { /* private mode */ } };
const uid = () => Math.random().toString(36).slice(2, 10);

// MARK: - Helpers

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function formatBytes(n) {
  if (!n) return '0 KB';
  if (n >= 1e9) return `${(n / 1e9).toFixed(2).replace('.', ',')} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1).replace('.', ',')} MB`;
  return `${Math.max(1, Math.round(n / 1e3))} KB`;
}

const totalSize = (files) => files.reduce((s, f) => s + (f.size || 0), 0);
const totalPages = (files) => files.reduce((s, f) => s + (f.pages || 0), 0);
const baseName = (path) => (path.split(/[\\/]/).pop() || '').replace(/\.[^.]+$/, '');

async function pickFiles(kind, multiple) {
  const filters = kind === 'pdf'
    ? [{ name: 'PDF', extensions: ['pdf'] }]
    : kind === 'image'
      ? [{ name: 'Resimler', extensions: ['jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'gif', 'bmp'] }]
      : [{ name: 'PDF ve resimler', extensions: ['pdf', 'jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'gif', 'bmp'] }];
  const picked = await tauri()?.dialog.open({ multiple, filters });
  if (!picked) return [];
  return Array.isArray(picked) ? picked : [picked];
}

// MARK: - Adding files

async function describe(paths) {
  if (!paths.length) return [];
  return (await invoke('file_info', { paths })).map((f) => ({ ...f, id: uid() }));
}

async function addFiles(toolId, paths) {
  const tool = toolById[toolId];
  const infos = (await describe(paths)).filter((f) => f.kind === tool.accepts);
  if (!infos.length) return;
  if (toolId === 'organize') return loadOrganize(infos[0]);
  const t = state.tools[toolId];
  const known = new Set(t.files.map((f) => f.path));
  const fresh = infos.filter((f) => !known.has(f.path));
  t.files = tool.single ? fresh.slice(0, 1).concat(fresh.length ? [] : t.files) : t.files.concat(fresh);
  t.job.results = [];
  t.job.error = null;
  render();
}

/** Files dropped on the window or opened with the app go to the current tool, or are routed like on macOS. */
async function routeFiles(paths) {
  const infos = await describe(paths);
  const pdfs = infos.filter((f) => f.kind === 'pdf').map((f) => f.path);
  const images = infos.filter((f) => f.kind === 'image').map((f) => f.path);
  if (!pdfs.length) {
    if (!images.length) return;
    state.view = 'images';
    return addFiles('images', images);
  }
  const target = state.view !== 'home' && state.view !== 'images' ? state.view : 'compress';
  state.view = target;
  return addFiles(target, pdfs);
}

// MARK: - Jobs

async function runJob(toolId, work) {
  const job = state.tools[toolId].job;
  if (job.running) return;
  Object.assign(job, newJob(), { id: uid(), running: true, status: 'Hazırlanıyor…' });
  jobsById.set(job.id, job);
  render();
  try {
    job.results = await work(job, (fraction, status) => { job.progress = fraction; job.status = status; refreshBar(); });
  } catch (error) {
    job.error = typeof error === 'string' ? error : error?.message || String(error);
  } finally {
    job.running = false;
    jobsById.delete(job.id);
    render();
  }
}

async function readPdf(path) {
  return new Uint8Array(await invoke('read_pdf', { path }));
}

const actions = {
  compress() {
    const t = state.tools.compress;
    const level = LEVELS.find((l) => l.id === t.level);
    runJob('compress', async (job, report) => {
      if (!t.scan) {
        return invoke('compress', { job: job.id, paths: t.files.map((f) => f.path), level: t.level, useGhostscript: state.settings.useGs, outDir: outDir() });
      }
      const results = [];
      for (const [i, file] of t.files.entries()) {
        try {
          const doc = await openPdf(await readPdf(file.path));
          const token = await invoke('scan_begin');
          for (let n = 1; n <= doc.numPages; n++) {
            report((i + n / doc.numPages) / t.files.length, `${file.name}: sayfa ${n} / ${doc.numPages}`);
            const { blob, widthPt, heightPt } = await renderPage(doc, n, { dpi: level.scanDpi, quality: level.jpeg });
            await invoke('scan_page', new Uint8Array(await blob.arrayBuffer()), {
              headers: { 'x-token': token, 'x-width': String(widthPt), 'x-height': String(heightPt) },
            });
          }
          closePdf(doc);
          results.push(await invoke('scan_finish', { token, path: file.path, outDir: outDir() }));
        } catch (error) {
          results.push({ title: file.name, detail: String(error?.message || error), success: false });
        }
      }
      return results;
    });
  },

  images() {
    const t = state.tools.images;
    runJob('images', (job) => invoke('images_to_pdf', { job: job.id, paths: t.files.map((f) => f.path), options: t.options, name: t.name, outDir: outDir() }));
  },

  merge() {
    const t = state.tools.merge;
    runJob('merge', (job) => invoke('merge', { job: job.id, paths: t.files.map((f) => f.path), name: t.name, outDir: outDir() }));
  },

  split() {
    const t = state.tools.split;
    const mode = { kind: t.mode, ranges: t.ranges, separate: t.separate, every: Number(t.every) || 1 };
    runJob('split', (job) => invoke('split', { job: job.id, path: t.files[0].path, mode, outDir: outDir() }));
  },

  organize() {
    const t = state.tools.organize;
    const pages = t.pages.map((p) => [p.index, p.rotation]);
    runJob('organize', () => invoke('organize', { path: t.file.path, pages, outDir: outDir() }));
  },

  pdfToImages() {
    const t = state.tools.pdfToImages;
    runJob('pdfToImages', async (job, report) => {
      const results = [];
      const png = t.format === 'png';
      for (const [i, file] of t.files.entries()) {
        try {
          const doc = await openPdf(await readPdf(file.path));
          const [token, folder] = await invoke('export_begin', { path: file.path, pages: doc.numPages, outDir: outDir() });
          for (let n = 1; n <= doc.numPages; n++) {
            report((i + n / doc.numPages) / t.files.length, `${file.name}: sayfa ${n} / ${doc.numPages}`);
            const { blob } = await renderPage(doc, n, { dpi: t.dpi, type: png ? 'image/png' : 'image/jpeg', quality: 0.9 });
            await invoke('export_page', new Uint8Array(await blob.arrayBuffer()), {
              headers: { 'x-token': token, 'x-index': String(n), 'x-ext': png ? 'png' : 'jpg' },
            });
          }
          await invoke('export_finish', { token });
          results.push({ title: folder.split(/[\\/]/).pop(), detail: `${doc.numPages} görsel · ${png ? 'PNG' : 'JPEG'}`, path: folder, isFolder: true, success: true });
          closePdf(doc);
        } catch (error) {
          results.push({ title: file.name, detail: String(error?.message || error), success: false });
        }
      }
      return results;
    });
  },

  protect() {
    const t = state.tools.protect;
    const args = { paths: t.files.map((f) => f.path), encrypt: t.mode === 'encrypt', password: t.password, ownerPassword: t.owner, outDir: outDir() };
    t.password = t.confirm = t.owner = '';
    runJob('protect', (job) => invoke('protect', { job: job.id, ...args }));
  },
};

// MARK: - Organizer

async function loadOrganize(info) {
  const t = state.tools.organize;
  closeOrganize();
  t.job = newJob();
  if (info.locked) {
    t.error = `"${info.name}" şifreli. Önce "PDF Şifrele" aracından şifresini kaldırın.`;
    return render();
  }
  try {
    t.doc = await openPdf(await readPdf(info.path));
    t.file = info;
    t.original = t.doc.numPages;
    t.pages = Array.from({ length: t.doc.numPages }, (_, i) => ({ id: uid(), index: i + 1, rotation: 0 }));
  } catch (error) {
    t.error = `"${info.name}" açılamadı. ${error?.message || ''}`;
  }
  render();
}

function closeOrganize() {
  const t = state.tools.organize;
  closePdf(t.doc);
  for (const url of pageThumbs.values()) URL.revokeObjectURL(url);
  pageThumbs.clear();
  Object.assign(t, { file: null, doc: null, pages: [], selected: new Set(), original: 0, error: null });
}

const organizeDirty = () => {
  const t = state.tools.organize;
  return t.pages.length !== t.original || t.pages.some((p, i) => p.index !== i + 1 || p.rotation !== 0);
};

const organizeOps = {
  rotateLeft: () => rotateSelected(-90),
  rotateRight: () => rotateSelected(90),
  deletePages() {
    const t = state.tools.organize;
    t.pages = t.pages.filter((p) => !t.selected.has(p.id));
    t.selected.clear();
    render();
  },
  selectAll() { const t = state.tools.organize; t.selected = new Set(t.pages.map((p) => p.id)); render(); },
  reverse() { state.tools.organize.pages.reverse(); render(); },
  reset() {
    const t = state.tools.organize;
    t.pages = Array.from({ length: t.original }, (_, i) => ({ id: uid(), index: i + 1, rotation: 0 }));
    t.selected.clear();
    render();
  },
  close() { closeOrganize(); render(); },
  async another() { const [path] = await pickFiles('pdf', false); if (path) addFiles('organize', [path]); },
};

function rotateSelected(degrees) {
  const t = state.tools.organize;
  for (const p of t.pages) if (t.selected.has(p.id)) p.rotation = (((p.rotation + degrees) % 360) + 360) % 360;
  render();
}

// MARK: - Rendering

const main = document.getElementById('main');

function renderNav() {
  const item = (id, label, iconHtml) =>
    `<button class="nav-item ${state.view === id ? 'active' : ''}" data-action="go" data-view="${id}" type="button">${iconHtml}<span>${label}</span></button>`;
  document.getElementById('nav').innerHTML =
    item('home', 'Ana sayfa', `<span class="home-ico">${icon('home')}</span>`) +
    '<div class="nav-section">Araçlar</div>' +
    TOOLS.map((t) => item(t.id, esc(t.title), toolIcon(t, 'sm'))).join('');
}

const toolIcon = (t, size) => `<span class="tool-icon ${size}" style="--tint:${t.tint}">${icon(t.icon)}</span>`;

function renderHome() {
  main.style.removeProperty('--tint');
  main.style.setProperty('--tint', 'var(--green)');
  main.innerHTML = `
    <div class="scroll"><div class="page wide">
      <div class="hero">
        <span class="badge">${icon('shield')}Çevrimdışı</span>
        <h1>Belge araçları</h1>
        <p>PDF dosyalarını küçültün, resimleri PDF'e çevirin, birleştirin ve düzenleyin. Tüm işlemler bu bilgisayarda yapılır; orijinal dosyalarınız asla değiştirilmez.</p>
      </div>
      ${dropzone({ title: 'Dosyaları buraya bırakın', text: "PDF'ler küçültmeye, resimler PDF'e dönüştürmeye gider\nveya seçmek için tıklayın", kind: 'any' })}
      <p class="eyebrow">Tüm araçlar</p>
      <div class="tool-grid">
        ${TOOLS.map((t) => `
          <button class="tool-card" style="--tint:${t.tint}" data-action="go" data-view="${t.id}" type="button">
            ${toolIcon(t, 'md')}
            <div><h3>${esc(t.title)}</h3><p>${esc(t.subtitle)}</p></div>
            <span class="arrow">${icon('arrowUpRight', 'width="16" height="16"')}</span>
          </button>`).join('')}
      </div>
      <div class="made-by"><strong>AZAR GOZALLI</strong><span>tarafından geliştirildi · Açık kaynak</span></div>
    </div></div>`;
}

function dropzone({ title, text, kind, compact = false, multiple = true }) {
  return `<button class="dropzone ${compact ? 'compact' : ''}" data-action="pick" data-kind="${kind}" data-multiple="${multiple}" type="button">
    <div>
      ${compact ? '' : `<div class="circle" style="margin:0 auto 12px">${icon('download')}</div>`}
      <strong>${compact ? icon('plus') : ''}${esc(title)}</strong><br>
      <span>${esc(text)}</span>
    </div>
  </button>`;
}

function fileList(toolId, { title = 'Dosyalar', sortable = false } = {}) {
  const t = state.tools[toolId];
  const showIndex = sortable && t.files.length > 1;
  return `<div class="card">
    <div class="card-head">
      <h2>${esc(title)}</h2>
      <span class="muted">${t.files.length} dosya · ${formatBytes(totalSize(t.files))}</span>
      <span class="spacer"></span>
      ${showIndex ? `<button class="link-btn" data-action="sortName" type="button">${icon('sort')}Ada göre sırala</button>` : ''}
      <button class="link-btn" data-action="clear" type="button">${icon('trash')}Temizle</button>
    </div>
    <div id="file-list">
      ${t.files.map((f, i) => `
        <div class="file-row" data-sort-item data-id="${f.id}">
          ${showIndex ? `<span class="index">${i + 1}</span>` : ''}
          <div class="thumb" data-thumb="${esc(f.path)}" data-kind="${f.kind}">${icon(f.kind === 'pdf' ? 'file' : 'image')}</div>
          <div class="file-meta">
            <strong title="${esc(f.path)}">${esc(f.name)}</strong>
            <span>${formatBytes(f.size)}${f.kind === 'pdf' || f.pages > 1 ? ` · ${f.locked ? '?' : f.pages} sayfa` : ''}${f.locked ? ` · <span class="lock">Şifreli</span>` : ''}</span>
          </div>
          ${showIndex ? `<span class="grip" data-grip title="Sürükleyerek sıralayın">${icon('grip')}</span>` : ''}
          <button class="icon-btn" data-action="remove" data-id="${f.id}" title="Listeden çıkar" type="button">${icon('x')}</button>
        </div>`).join('')}
    </div>
  </div>`;
}

function filePicker(toolId, { title, text, listTitle, sortable }) {
  const tool = toolById[toolId];
  const t = state.tools[toolId];
  const multiple = !tool.single;
  if (!t.files.length) return dropzone({ title, text: `${text}\nveya seçmek için tıklayın`, kind: tool.accepts, multiple });
  return fileList(toolId, { title: listTitle, sortable }) +
    dropzone({ title: multiple ? 'Daha fazla ekle' : 'Başka bir dosya seç', text: 'Sürükleyip bırakın veya tıklayın', kind: tool.accepts, compact: true, multiple });
}

const segmented = (key, value, options, disabled = false) =>
  `<div class="segmented">${options.map(([v, label]) =>
    `<button type="button" class="${String(v) === String(value) ? 'on' : ''}" data-action="set" data-key="${key}" data-value="${v}" ${disabled ? 'disabled' : ''}>${esc(label)}</button>`).join('')}</div>`;

const banner = (text, kind = 'info') =>
  `<div class="banner ${kind === 'info' ? '' : kind}">${icon(kind === 'info' ? 'info' : kind === 'key' ? 'key' : 'warn')}<span>${esc(text)}</span></div>`;

function results(job) {
  let html = job.error ? banner(job.error, 'error') : '';
  if (!job.results.length) return html;
  const allOk = job.results.every((r) => r.success);
  const firstPath = job.results.find((r) => r.path)?.path;
  html += `<div class="card results">
    <div class="card-head ${allOk ? '' : 'partial'}">
      ${icon(allOk ? 'checkCircle' : 'info')}<h2>${allOk ? 'Tamamlandı' : 'İşlem bitti'}</h2><span class="spacer"></span>
      ${firstPath ? `<button class="link-btn" data-action="reveal" data-path="${esc(firstPath)}" type="button">${icon('folder')}Klasörde göster</button>` : ''}
      <button class="link-btn" data-action="dismiss" type="button">Kapat</button>
    </div>
    ${job.results.map((r) => `
      <div class="result-row ${r.success ? '' : 'failed'}">
        ${icon(r.success ? (r.isFolder ? 'folder' : 'file') : 'warn')}
        <div class="file-meta"><strong>${esc(r.title)}</strong><span>${esc(r.detail)}</span></div>
        ${r.originalSize && r.newSize ? `<span class="sizes"><s>${formatBytes(r.originalSize)}</s>${icon('arrowRight')}<b>${formatBytes(r.newSize)}</b></span>` : ''}
        ${r.path ? `<button class="btn" data-action="open" data-path="${esc(r.path)}" type="button">Aç</button>
          <button class="icon-btn" data-action="reveal" data-path="${esc(r.path)}" title="Klasörde göster" type="button">${icon('search')}</button>` : ''}
      </div>`).join('')}
  </div>`;
  return html;
}

function toolBody(id) {
  const t = state.tools[id];
  switch (id) {
    case 'compress': {
      const warn = t.level !== 'balanced' || t.scan;
      return filePicker(id, { title: "Küçültülecek PDF'leri buraya sürükleyin", text: 'Birden fazla PDF seçebilirsiniz · Orijinal dosyalar değişmez' }) + `
        <div class="card pad">
          <p class="eyebrow">Sıkıştırma seviyesi</p>
          <div class="choices">${LEVELS.map((l) => `
            <button type="button" class="choice ${t.level === l.id ? 'selected' : ''}" data-action="set" data-key="level" data-value="${l.id}">
              <span class="top"><strong>${l.title}</strong><span class="dot">${t.level === l.id ? icon('check') : ''}</span></span>
              <span>${l.text}</span>
            </button>`).join('')}
          </div>
          <label class="toggle">
            <span><strong>Taranmış belge modu</strong><span>Her sayfayı görüntüye çevirerek tarama ve fotoğraf içeren PDF'lerde çok daha fazla küçültür. Metin seçilemez hale gelir.</span></span>
            <span class="switch"><input type="checkbox" data-bind="scan" ${t.scan ? 'checked' : ''}><i></i></span>
          </label>
          ${warn
            ? banner('Sıkıştırmayı artırmak görüntü kalitesini düşürebilir. Sonucu kontrol etmeden orijinal dosyayı silmeyin.', 'warn')
            : banner("Makul seçenek dosyayı bozmadan küçültmeyi hedefler. Zaten optimize edilmiş PDF'lerde boyut her zaman azalmayabilir; bu durumda yeni dosya oluşturulmaz.")}
        </div>`;
    }
    case 'images': {
      const o = t.options;
      return filePicker(id, { title: 'Resimleri buraya sürükleyin', text: 'JPG, PNG, WEBP, TIFF, GIF, BMP · Sırayı sürükleyerek değiştirin', listTitle: 'Sayfa sırası', sortable: true }) + `
        <div class="card pad">
          <p class="eyebrow">Sayfa ayarları</p>
          <div class="option-row"><span class="label">Sayfa boyutu</span>${segmented('options.pageSize', o.pageSize, [['fit', 'Resme göre'], ['a4', 'A4'], ['a5', 'A5'], ['letter', 'Letter'], ['legal', 'Legal']])}</div>
          <div class="option-row"><span class="label">Yönlendirme</span>${segmented('options.orientation', o.orientation, [['auto', 'Otomatik'], ['portrait', 'Dikey'], ['landscape', 'Yatay']], o.pageSize === 'fit')}</div>
          <div class="option-row"><span class="label">Kenar boşluğu</span>${segmented('options.margin', o.margin, [['none', 'Yok'], ['small', 'Dar'], ['normal', 'Normal']])}</div>
          <div class="option-row"><span class="label">Görüntü kalitesi</span>${segmented('options.quality', o.quality, [['original', 'Orijinal'], ['high', 'Yüksek'], ['balanced', 'Dengeli'], ['small', 'Küçük dosya']])}</div>
          <div class="option-row"><label for="f-name">Dosya adı</label><span class="inline"><input id="f-name" class="field" type="text" data-bind="name" value="${esc(t.name)}" placeholder="${esc(t.files[0] ? baseName(t.files[0].path) : 'Belge')}"><span class="muted">.pdf</span></span></div>
          ${banner('HEIC fotoğrafları için önce JPG olarak dışa aktarın (Fotoğraflar uygulamasında "Farklı kaydet").')}
        </div>`;
    }
    case 'merge':
      return filePicker(id, { title: "Birleştirilecek PDF'leri sürükleyin", text: 'En az iki PDF · Sırayı sürükleyerek değiştirin', listTitle: 'Birleştirme sırası', sortable: true }) + `
        <div class="card pad">
          <p class="eyebrow">Çıktı</p>
          <div class="option-row"><label for="f-name">Dosya adı</label><span class="inline"><input id="f-name" class="field" type="text" data-bind="name" value="${esc(t.name)}" placeholder="Birleştirilmiş belge"><span class="muted">.pdf</span></span></div>
        </div>`;
    case 'split': {
      let body;
      if (t.mode === 'ranges') {
        body = `<div class="option-row"><label for="f-ranges">Sayfalar</label><input id="f-ranges" class="field" type="text" data-bind="ranges" value="${esc(t.ranges)}" placeholder="Örn. 1-3, 5, 8-10"></div>
          <label class="toggle"><span><strong>Her aralığı ayrı dosya yap</strong><span>${t.separate ? 'Virgülle ayrılan her aralık ayrı PDF olur.' : "Seçilen tüm sayfalar tek bir PDF'te toplanır."}</span></span>
          <span class="switch"><input type="checkbox" data-bind="separate" data-rerender ${t.separate ? 'checked' : ''}><i></i></span></label>`;
      } else if (t.mode === 'every') {
        body = `<div class="option-row"><label for="f-every">Sayfa sayısı</label><span class="inline"><input id="f-every" class="field" type="number" min="1" max="999" style="width:90px" data-bind="every" value="${esc(t.every)}"><span class="muted">sayfada bir yeni dosya</span></span></div>`;
      } else {
        body = '<p class="muted" style="margin:0">Her sayfa ayrı bir PDF dosyası olarak yeni bir klasöre kaydedilir.</p>';
      }
      return filePicker(id, { title: "Bölünecek PDF'i sürükleyin", text: 'Tek dosya' }) + `
        <div class="card pad"><p class="eyebrow">Bölme yöntemi</p>
          ${segmented('mode', t.mode, [['each', 'Her sayfa ayrı'], ['ranges', 'Sayfa seç'], ['every', 'Her N sayfada']])}
          ${body}
        </div>`;
    }
    case 'organize':
      return organizeBody();
    case 'pdfToImages':
      return filePicker(id, { title: "PDF'leri buraya sürükleyin", text: 'Her PDF için ayrı bir görsel klasörü oluşturulur' }) + `
        <div class="card pad"><p class="eyebrow">Görsel ayarları</p>
          <div class="option-row"><span class="label">Biçim</span>${segmented('format', t.format, [['png', 'PNG'], ['jpeg', 'JPEG']])}</div>
          <div class="option-row"><span class="label">Çözünürlük</span>${segmented('dpi', t.dpi, [[72, '72 DPI · Ekran'], [150, '150 DPI · Standart'], [300, '300 DPI · Baskı']])}</div>
        </div>`;
    case 'protect': {
      const enc = t.mode === 'encrypt';
      return filePicker(id, { title: "PDF'leri buraya sürükleyin", text: 'Aynı şifre seçilen tüm dosyalara uygulanır' }) + `
        <div class="card pad">
          ${segmented('mode', t.mode, [['encrypt', 'Şifre ekle'], ['decrypt', 'Şifre kaldır']])}
          ${enc ? `
            <div class="option-row"><label for="f-pw">Açılış şifresi</label><input id="f-pw" class="field" type="password" data-bind="password" placeholder="Şifre" autocomplete="new-password"></div>
            <div class="option-row"><label for="f-pw2">Şifre (tekrar)</label><span class="inline"><input id="f-pw2" class="field" type="password" data-bind="confirm" placeholder="Şifreyi tekrar girin" autocomplete="new-password"><span class="mismatch" id="mismatch"></span></span></div>
            <div class="option-row"><label for="f-owner">Sahip şifresi</label><input id="f-owner" class="field" type="password" data-bind="owner" placeholder="İsteğe bağlı" autocomplete="new-password"></div>
            ${banner('Şifreyi unutursanız dosya açılamaz; şifre hiçbir yerde saklanmaz. Orijinal dosyanız şifresiz olarak yerinde kalır. Şifreleme: AES-256.', 'key')}`
          : `<div class="option-row"><label for="f-pw">Mevcut şifre</label><input id="f-pw" class="field" type="password" data-bind="password" placeholder="PDF'in şifresi" autocomplete="off"></div>
            <p class="muted" style="margin:0">Şifresini bildiğiniz PDF'lerin şifresiz bir kopyası oluşturulur.</p>`}
        </div>`;
    }
    default:
      return '';
  }
}

function organizeBody() {
  const t = state.tools.organize;
  let html = t.error ? banner(t.error, 'error') : '';
  if (!t.file) {
    return html + dropzone({ title: "Düzenlenecek PDF'i sürükleyin", text: 'Tek dosya\nveya seçmek için tıklayın', kind: 'pdf', multiple: false });
  }
  const none = t.selected.size === 0;
  html += `<div class="card org-bar">
    <div class="thumb" data-thumb="${esc(t.file.path)}" data-kind="pdf">${icon('file')}</div>
    <div class="file-meta"><strong>${esc(t.file.name)}</strong><span>Tıklayarak seçin, Ctrl ile çoklu seçim · Sürükleyerek sıralayın · Delete ile silin</span></div>
    <div class="tools">
      <button class="icon-btn" data-org="rotateLeft" title="Sola döndür" ${none ? 'disabled' : ''} type="button">${icon('rotateLeft')}</button>
      <button class="icon-btn" data-org="rotateRight" title="Sağa döndür" ${none ? 'disabled' : ''} type="button">${icon('rotateRight')}</button>
      <button class="icon-btn danger" data-org="deletePages" title="Seçili sayfaları sil" ${none ? 'disabled' : ''} type="button">${icon('trash')}</button>
      <button class="icon-btn" data-org="selectAll" title="Tümünü seç" type="button">${icon('checkCircle')}</button>
      <button class="icon-btn" data-org="reverse" title="Sırayı ters çevir" type="button">${icon('reverse')}</button>
      <button class="icon-btn" data-org="reset" title="Değişiklikleri geri al" type="button">${icon('undo')}</button>
      <button class="icon-btn" data-org="another" title="Başka PDF aç" type="button">${icon('folder')}</button>
      <button class="icon-btn" data-org="close" title="Kapat" type="button">${icon('x')}</button>
    </div>
  </div>
  <div class="page-grid" id="page-grid">${pageTiles()}</div>`;
  return html;
}

function pageTiles() {
  const t = state.tools.organize;
  return t.pages.map((p, i) => {
    const url = pageThumbs.get(p.index);
    return `<div class="page-tile ${t.selected.has(p.id) ? 'selected' : ''}" data-sort-item data-tile data-id="${p.id}" data-index="${p.index}">
      <div class="frame">
        ${url ? `<img src="${url}" alt="" data-rot="${p.rotation}" draggable="false">` : '<span class="loader"></span>'}
        <span class="check">${icon('check')}</span>
      </div>
      <div class="label"><b>${i + 1}</b>${p.index !== i + 1 ? ` <span>(asıl ${p.index})</span>` : ''}${p.rotation ? ` <span>· ${p.rotation}°</span>` : ''}</div>
    </div>`;
  }).join('');
}

function actionBarHtml(id) {
  const tool = toolById[id];
  const t = state.tools[id];
  const job = t.job;
  const label = { compress: 'PDF Küçült', images: 'PDF Oluştur', merge: 'Birleştir', split: 'Böl', organize: 'Kaydet',
    pdfToImages: 'Görsellere Dönüştür', protect: t.mode === 'encrypt' ? 'Şifrele' : 'Şifreyi Kaldır' }[id];
  let info = '';
  let disabled = false;
  switch (id) {
    case 'compress': info = t.files.length ? `${t.files.length} PDF · ${formatBytes(totalSize(t.files))}` : 'PDF ekleyin'; disabled = !t.files.length; break;
    case 'images': info = t.files.length ? `${totalPages(t.files)} sayfa oluşturulacak` : 'Resim ekleyin'; disabled = !t.files.length; break;
    case 'merge': info = t.files.length < 2 ? 'En az iki PDF ekleyin' : `${t.files.length} dosya · ${totalPages(t.files)} sayfa`; disabled = t.files.length < 2; break;
    case 'split': info = t.files[0] ? `${t.files[0].pages} sayfa` : 'PDF ekleyin'; disabled = !t.files.length || (t.mode === 'ranges' && !t.ranges.trim()); break;
    case 'organize': info = t.file ? `${t.pages.length} sayfa · ${t.selected.size} seçili` : 'PDF ekleyin'; disabled = !t.file || !t.pages.length || !organizeDirty(); break;
    case 'pdfToImages': info = t.files.length ? `${totalPages(t.files)} sayfa dışa aktarılacak` : 'PDF ekleyin'; disabled = !t.files.length; break;
    case 'protect': info = t.files.length ? `${t.files.length} PDF` : 'PDF ekleyin'; disabled = !t.files.length || !t.password || (t.mode === 'encrypt' && t.password !== t.confirm); break;
  }
  const left = job.running
    ? `<div class="progress"><i style="width:${Math.round(job.progress * 100)}%"></i></div><span class="info">${esc(job.status)}</span>`
    : `<span class="info">${esc(info)}</span>`;
  return `<div class="inner">${left}
    <button class="btn big" data-action="start" ${disabled || job.running ? 'disabled' : ''} type="button">
      ${job.running ? '<span class="spinner"></span>' : icon(tool.icon)}${esc(label)}
    </button></div>`;
}

function refreshBar() {
  const bar = document.getElementById('action-bar');
  if (bar && state.view !== 'home') bar.innerHTML = actionBarHtml(state.view);
  const mismatch = document.getElementById('mismatch');
  if (mismatch) {
    const t = state.tools.protect;
    mismatch.textContent = t.confirm && t.confirm !== t.password ? 'Eşleşmiyor' : '';
  }
}

function renderTool(id) {
  const tool = toolById[id];
  main.style.setProperty('--tint', tool.tint);
  const scrollTop = main.querySelector('.scroll')?.scrollTop ?? 0;
  const sameView = main.dataset.view === id;
  main.innerHTML = `
    <div class="scroll"><div class="page">
      <div class="tool-head">
        ${toolIcon(tool, 'lg')}
        <div><h1>${esc(tool.title)}</h1><p>${esc(tool.subtitle)}</p></div>
        <span class="badge">${icon('shield')}Çevrimdışı</span>
      </div>
      ${toolBody(id)}
      ${results(state.tools[id].job)}
    </div></div>
    <div class="action-bar" id="action-bar">${actionBarHtml(id)}</div>`;
  if (sameView) main.querySelector('.scroll').scrollTop = scrollTop;
}

function render() {
  renderNav();
  if (state.view === 'home') renderHome(); else renderTool(state.view);
  main.dataset.view = state.view;
  hydrateThumbs();
  hydratePages();
  applyRotations();
}

// MARK: - Thumbnails

let thumbQueue = Promise.resolve();

function hydrateThumbs() {
  for (const el of main.querySelectorAll('[data-thumb]')) {
    const path = el.dataset.thumb;
    if (thumbCache.has(path)) {
      if (thumbCache.get(path)) el.innerHTML = `<img src="${thumbCache.get(path)}" alt="">`;
      continue;
    }
    thumbQueue = thumbQueue.then(async () => {
      if (thumbCache.has(path)) return;
      try {
        let blob;
        if (el.dataset.kind === 'pdf') {
          const doc = await openPdf(await readPdf(path));
          blob = (await renderPage(doc, 1, { dpi: 72, maxSide: 160, quality: 0.8 })).blob;
          closePdf(doc);
        } else {
          blob = new Blob([await invoke('thumbnail', { path })], { type: 'image/jpeg' });
        }
        thumbCache.set(path, URL.createObjectURL(blob));
      } catch (error) {
        console.warn('Önizleme oluşturulamadı', path, error);
        thumbCache.set(path, '');
      }
      for (const target of main.querySelectorAll(`[data-thumb="${CSS.escape(path)}"]`)) {
        if (thumbCache.get(path)) target.innerHTML = `<img src="${thumbCache.get(path)}" alt="">`;
      }
    });
  }
}

/** Renders organizer thumbnails one after another (no IntersectionObserver: it pauses in hidden windows). */
let pageRenderToken = 0;

async function hydratePages() {
  const t = state.tools.organize;
  if (state.view !== 'organize' || !t.doc) return;
  const token = ++pageRenderToken;
  const doc = t.doc;
  const pending = t.pages.map((p) => p.index).filter((index) => !pageThumbs.has(index));
  for (const index of pending) {
    if (token !== pageRenderToken || doc !== t.doc) return;
    if (pageThumbs.has(index)) continue;
    try {
      const { blob } = await renderPage(doc, index, { dpi: 72, maxSide: 300, quality: 0.8 });
      if (doc !== t.doc) return;
      pageThumbs.set(index, URL.createObjectURL(blob));
    } catch (error) {
      console.warn('Sayfa önizlemesi oluşturulamadı', index, error);
      continue;
    }
    for (const tile of main.querySelectorAll(`[data-tile][data-index="${index}"]`)) {
      if (tile.querySelector('img')) continue;
      const page = t.pages.find((p) => p.id === tile.dataset.id);
      tile.querySelector('.frame').insertAdjacentHTML('afterbegin', `<img src="${pageThumbs.get(index)}" alt="" data-rot="${page?.rotation || 0}" draggable="false">`);
      tile.querySelector('.loader')?.remove();
    }
    applyRotations();
  }
}

/** Rotated thumbnails are scaled so they still fit inside their frame. */
function applyRotations() {
  for (const img of main.querySelectorAll('img[data-rot]')) {
    const apply = () => {
      const rot = Number(img.dataset.rot) || 0;
      const quarter = rot % 180 !== 0;
      const ratio = quarter ? Math.min(img.naturalWidth, img.naturalHeight) / Math.max(img.naturalWidth, img.naturalHeight) : 1;
      img.style.transform = rot ? `rotate(${rot}deg) scale(${ratio || 1})` : '';
    };
    if (img.complete) apply(); else img.addEventListener('load', apply, { once: true });
  }
}

// MARK: - Pointer-based reordering (HTML5 drag & drop is unavailable while file drops are enabled)

let drag = null;

function listFor(view) {
  return view === 'organize' ? state.tools.organize.pages : state.tools[view]?.files;
}

main.addEventListener('pointerdown', (event) => {
  if (event.button !== 0) return;
  const tile = event.target.closest('[data-tile]');
  const grip = event.target.closest('[data-grip]');
  const item = tile || grip?.closest('[data-sort-item]');
  if (!item || state.tools[state.view]?.job.running) return;
  drag = { id: item.dataset.id, x: event.clientX, y: event.clientY, moved: false, tile: !!tile, extend: event.ctrlKey || event.metaKey || event.shiftKey };
  if (grip) event.preventDefault();
});

window.addEventListener('pointermove', (event) => {
  if (!drag) return;
  if (!drag.moved && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 6) return;
  drag.moved = true;
  const list = listFor(state.view);
  if (!list) return;
  const under = document.elementFromPoint(event.clientX, event.clientY)?.closest('[data-sort-item]');
  if (under && under.dataset.id !== drag.id) {
    const from = list.findIndex((x) => x.id === drag.id);
    const to = list.findIndex((x) => x.id === under.dataset.id);
    if (from >= 0 && to >= 0) {
      const [moved] = list.splice(from, 1);
      list.splice(to, 0, moved);
      redrawSortable();
    }
  }
  for (const el of main.querySelectorAll('[data-sort-item]')) el.classList.toggle('dragging', el.dataset.id === drag.id);
});

window.addEventListener('pointerup', () => {
  if (!drag) return;
  const { moved, tile, id, extend } = drag;
  drag = null;
  if (moved) return render();
  if (tile) {
    const sel = state.tools.organize.selected;
    if (extend) { if (sel.has(id)) sel.delete(id); else sel.add(id); }
    else if (sel.size === 1 && sel.has(id)) sel.clear();
    else { sel.clear(); sel.add(id); }
    render();
  }
});

function redrawSortable() {
  if (state.view === 'organize') {
    document.getElementById('page-grid').innerHTML = pageTiles();
    applyRotations();
  } else {
    const t = state.tools[state.view];
    const list = document.getElementById('file-list');
    if (!list) return;
    const rows = new Map([...list.children].map((row) => [row.dataset.id, row]));
    t.files.forEach((f, i) => {
      const row = rows.get(f.id);
      row.querySelector('.index').textContent = String(i + 1);
      list.appendChild(row);
    });
  }
}

// MARK: - Events

document.addEventListener('click', async (event) => {
  const org = event.target.closest('[data-org]');
  if (org) return organizeOps[org.dataset.org]?.();
  const el = event.target.closest('[data-action]');
  if (!el) return;
  const view = state.view;
  const t = state.tools[view];
  switch (el.dataset.action) {
    case 'go':
      state.view = el.dataset.view;
      render();
      main.querySelector('.scroll')?.scrollTo(0, 0);
      break;
    case 'pick': {
      const kind = el.dataset.kind;
      const paths = await pickFiles(kind, el.dataset.multiple !== 'false');
      if (!paths.length) return;
      if (kind === 'any') routeFiles(paths); else addFiles(view, paths);
      break;
    }
    case 'remove':
      t.files = t.files.filter((f) => f.id !== el.dataset.id);
      render();
      break;
    case 'clear':
      t.files = [];
      render();
      break;
    case 'sortName':
      t.files.sort((a, b) => a.name.localeCompare(b.name, 'tr', { numeric: true }));
      render();
      break;
    case 'set': {
      const keys = el.dataset.key.split('.');
      const target = keys.length > 1 ? t[keys[0]] : t;
      const key = keys[keys.length - 1];
      target[key] = typeof target[key] === 'number' ? Number(el.dataset.value) : el.dataset.value;
      render();
      break;
    }
    case 'start':
      actions[view]?.();
      break;
    case 'dismiss':
      t.job.results = [];
      t.job.error = null;
      render();
      break;
    case 'open':
      invoke('open_path', { path: el.dataset.path }).catch(() => {});
      break;
    case 'reveal':
      invoke('reveal_path', { path: el.dataset.path }).catch(() => {});
      break;
  }
});

main.addEventListener('input', (event) => {
  const key = event.target.dataset?.bind;
  if (!key) return;
  const t = state.tools[state.view];
  const input = event.target;
  t[key] = input.type === 'checkbox' ? input.checked : input.type === 'number' ? Number(input.value) : input.value;
  if (input.type === 'checkbox' || input.dataset.rerender !== undefined) render(); else refreshBar();
});

document.addEventListener('keydown', (event) => {
  if (state.view !== 'organize' || event.target.closest('input, dialog')) return;
  const t = state.tools.organize;
  if ((event.key === 'Delete' || event.key === 'Backspace') && t.selected.size) { event.preventDefault(); organizeOps.deletePages(); }
  if (event.key.toLowerCase() === 'a' && (event.ctrlKey || event.metaKey) && t.file) { event.preventDefault(); organizeOps.selectAll(); }
});

// MARK: - Settings & about

const settingsDialog = document.getElementById('settings-dialog');

function syncSettingsDialog() {
  const s = state.settings;
  for (const radio of settingsDialog.querySelectorAll('input[name="outMode"]')) radio.checked = radio.value === s.outMode;
  document.getElementById('folder-row').hidden = s.outMode !== 'folder';
  document.getElementById('folder-path').textContent = s.outDir || 'Klasör seçilmedi';
  document.getElementById('use-gs').checked = s.useGs;
}

document.getElementById('open-settings').addEventListener('click', async () => {
  syncSettingsDialog();
  const gs = await invoke('ghostscript').catch(() => null);
  document.getElementById('gs-status').textContent = gs
    ? `Bulundu: ${gs}`
    : 'Ghostscript bulunamadı; yerleşik motor kullanılır. Daha güçlü sıkıştırma için isteğe bağlı olarak ghostscript.com adresinden kurabilirsiniz.';
  settingsDialog.showModal();
});

settingsDialog.addEventListener('change', (event) => {
  if (event.target.name === 'outMode') state.settings.outMode = event.target.value;
  if (event.target.id === 'use-gs') state.settings.useGs = event.target.checked;
  saveSettings();
  syncSettingsDialog();
});

document.getElementById('choose-folder').addEventListener('click', async () => {
  const folder = await tauri()?.dialog.open({ directory: true });
  if (folder) { state.settings.outDir = folder; saveSettings(); syncSettingsDialog(); }
});

document.getElementById('open-about').addEventListener('click', () => document.getElementById('about-dialog').showModal());

// MARK: - Boot

for (const el of document.querySelectorAll('[data-icon]')) el.innerHTML = icon(el.dataset.icon);

async function boot() {
  render();
  const api = tauri();
  if (!api) return;
  try {
    const version = await api.app.getVersion();
    document.getElementById('version').textContent = version;
    document.getElementById('about-version').textContent = version;
  } catch { /* keep default */ }
  await api.event.listen('progress', ({ payload }) => {
    const job = jobsById.get(payload.job);
    if (!job) return;
    job.progress = payload.fraction;
    job.status = payload.status;
    refreshBar();
  });
  await api.webviewWindow.getCurrentWebviewWindow().onDragDropEvent(({ payload }) => {
    if (payload.type === 'enter' || payload.type === 'over') document.body.classList.add('dragging');
    else document.body.classList.remove('dragging');
    if (payload.type === 'drop' && payload.paths?.length) routeFiles(payload.paths);
  });
  const initial = await invoke('initial_files').catch(() => []);
  if (initial.length) routeFiles(initial);
}

boot();
