// Page rendering with the bundled pdf.js (no network). Used for thumbnails, PDF→image and scan mode.
import * as pdfjs from './vendor/pdfjs/pdf.min.mjs';

pdfjs.GlobalWorkerOptions.workerSrc = './vendor/pdfjs/pdf.worker.min.mjs';

const MAX_SIDE = 10000;
// The worker resolves relative URLs against its own location, so pass absolute ones.
const asset = (path) => new URL(`./vendor/pdfjs/${path}`, document.baseURI).href;

export async function openPdf(bytes) {
  return pdfjs.getDocument({
    data: bytes,
    isEvalSupported: false,
    enableXfa: false,
    enableScripting: false,
    cMapUrl: asset('cmaps/'),
    cMapPacked: true,
    standardFontDataUrl: asset('standard_fonts/'),
    wasmUrl: asset('wasm/'),
    iccUrl: asset('iccs/'),
  }).promise;
}

/**
 * Renders one page (1-based) and returns encoded image bytes plus the page size in points.
 * `rotation` is added to the page's own rotation.
 */
export async function renderPage(doc, number, { dpi = 150, rotation = 0, type = 'image/jpeg', quality = 0.85, maxSide = 0 } = {}) {
  const page = await doc.getPage(number);
  const angle = (((page.rotate + rotation) % 360) + 360) % 360;
  const base = page.getViewport({ scale: 1, rotation: angle });
  let scale = dpi / 72;
  const limit = maxSide || MAX_SIDE;
  scale = Math.min(scale, limit / Math.max(base.width, base.height));
  const viewport = page.getViewport({ scale, rotation: angle });
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(viewport.width));
  canvas.height = Math.max(1, Math.round(viewport.height));
  // "print" intent renders without requestAnimationFrame, so work continues while the window is minimised.
  await page.render({ canvas, viewport, background: '#ffffff', intent: 'print' }).promise;
  const blob = await new Promise((resolve) => canvas.toBlob(resolve, type, quality));
  page.cleanup();
  canvas.width = canvas.height = 0;
  return { blob, widthPt: base.width, heightPt: base.height };
}

/** Frees a document and its worker-side resources. */
export function closePdf(doc) {
  try { doc?.loadingTask?.destroy(); } catch { /* already closed */ }
}
