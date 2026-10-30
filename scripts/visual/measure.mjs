// Browser driver of the visual-v1 profile; scripts/visual.py owns the corpus,
// the comparison and the baseline. Input: a JSON job file
//   { "pairs": [{ "id", "ruby", "adoctra" }], "viewports": [{ "name", "width", "height" }],
//     "out": "<screenshot dir>", "threshold": 0.1, "jobs": 4 }
// Output (stdout): one JSON object per pair and viewport.
//
// Every non-blank word rendered in the page is measured with a DOM Range: its
// text, box, the computed text style of its element and the effective
// background. Generated content (::before/::marker) has no text node and is
// only visible to the pixel comparison. Pages load offline: any request that
// is not a file: URL is aborted and reported, on both sides alike.
import { chromium } from 'playwright-core';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const STYLE = ['font-family', 'font-size', 'font-weight', 'font-style', 'color',
  'text-decoration-line', 'text-transform', 'letter-spacing', 'font-variant-caps'];

function extract(props) {
  const styles = [];
  const index = new Map();
  const words = [];
  // Every painted background from the element up to the page, innermost
  // first; visual.py composites them (and the text colour) over white.
  const backgrounds = (el) => {
    const chain = [];
    for (let e = el; e; e = e.parentElement) {
      const c = getComputedStyle(e).backgroundColor;
      if (c !== 'rgba(0, 0, 0, 0)' && c !== 'transparent') chain.push(c);
    }
    return chain.join(' | ');
  };
  const context = (el) => {
    const parts = [];
    for (let e = el; e && e !== document.body && parts.length < 3; e = e.parentElement) {
      let s = e.tagName.toLowerCase();
      if (e.classList.length) s += '.' + [...e.classList].join('.');
      parts.unshift(s);
    }
    return parts.join('>');
  };
  const round = (v) => Math.round(v * 10) / 10;
  const range = document.createRange();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node; (node = walker.nextNode());) {
    const el = node.parentElement;
    if (!el || el.closest('script, style, template')) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility !== 'visible') continue;
    const tuple = JSON.stringify([...props.map((p) => cs.getPropertyValue(p)), backgrounds(el), context(el)]);
    let style = index.get(tuple);
    if (style === undefined) {
      style = styles.length;
      index.set(tuple, style);
      styles.push(JSON.parse(tuple));
    }
    const re = /\S+/g;
    for (let m; (m = re.exec(node.data));) {
      range.setStart(node, m.index);
      range.setEnd(node, m.index + m[0].length);
      const box = range.getClientRects()[0];
      if (!box || (box.width === 0 && box.height === 0)) continue;
      words.push([m[0], round(box.left + scrollX), round(box.top + scrollY), round(box.width), round(box.height), style]);
    }
  }
  return {
    width: document.documentElement.scrollWidth,
    height: document.documentElement.scrollHeight,
    styles,
    words,
  };
}

// pixelmatch's YIQ colour distance without anti-aliasing detection; both
// images are placed on a white canvas of the larger size.
function compare(a, b, threshold) {
  const load = (src) => new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = src;
  });
  return Promise.all([load(a), load(b)]).then(([ia, ib]) => {
    const w = Math.max(ia.width, ib.width);
    const h = Math.max(ia.height, ib.height);
    const pixels = (img) => {
      const c = document.createElement('canvas');
      c.width = w;
      c.height = h;
      const g = c.getContext('2d');
      g.fillStyle = '#fff';
      g.fillRect(0, 0, w, h);
      g.drawImage(img, 0, 0);
      return g.getImageData(0, 0, w, h).data;
    };
    const pa = pixels(ia);
    const pb = pixels(ib);
    const out = document.createElement('canvas');
    out.width = w;
    out.height = h;
    const og = out.getContext('2d');
    const diff = og.createImageData(w, h);
    const out8 = diff.data;
    const max = 35215 * threshold * threshold;
    let different = 0;
    for (let i = 0; i < pa.length; i += 4) {
      const dr = pa[i] - pb[i];
      const dg = pa[i + 1] - pb[i + 1];
      const db = pa[i + 2] - pb[i + 2];
      const y = dr * 0.29889531 + dg * 0.58662247 + db * 0.11448223;
      const iq = dr * 0.59597799 - dg * 0.2741761 - db * 0.32180189;
      const q = dr * 0.21147017 - dg * 0.52261711 + db * 0.31114694;
      const delta = 0.5053 * y * y + 0.299 * iq * iq + 0.1957 * q * q;
      if (delta > max) {
        different++;
        out8[i] = 255;
        out8[i + 1] = 0;
        out8[i + 2] = 0;
      } else {
        const grey = 255 - (255 - (pa[i] + pa[i + 1] + pa[i + 2]) / 3) * 0.1;
        out8[i] = out8[i + 1] = out8[i + 2] = grey;
      }
      out8[i + 3] = 255;
    }
    og.putImageData(diff, 0, 0);
    return { width: w, height: h, different, total: w * h, image: out.toDataURL('image/png') };
  });
}

async function render(browser, file, viewport, shot) {
  const context = await browser.newContext({
    viewport: { width: viewport.width, height: viewport.height },
    deviceScaleFactor: 1,
    colorScheme: 'light',
    reducedMotion: 'reduce',
    locale: 'en-US',
    timezoneId: 'UTC',
  });
  const blocked = [];
  await context.route('**/*', (route) => {
    const url = route.request().url();
    if (url.startsWith('file:') || url.startsWith('data:')) return route.continue();
    blocked.push(url);
    return route.abort();
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(pathToFileURL(file).href, { waitUntil: 'load' });
  await page.evaluate(() => document.fonts.ready);
  // Native media controls draw a loading state until the element settles
  // (metadata or a load error); a screenshot in between is nondeterministic.
  await page.evaluate(() => Promise.all([...document.querySelectorAll('video, audio')].map((media) =>
    new Promise((resolve) => {
      const settled = () => media.error || media.readyState >= 1
        || media.networkState === HTMLMediaElement.NETWORK_NO_SOURCE;
      if (settled()) return resolve();
      const timer = setInterval(() => { if (settled()) { clearInterval(timer); resolve(); } }, 20);
      setTimeout(() => { clearInterval(timer); resolve(); }, 3000);
    }))));
  const result = await page.evaluate(extract, STYLE);
  // The inside of native media players and frames is the browser's, not a
  // stylesheet's, and animates while loading: a solid mask keeps their box.
  await page.screenshot({
    path: shot, fullPage: true, animations: 'disabled', caret: 'hide',
    mask: [page.locator('video, audio, iframe')], maskColor: '#ff00ff',
  });
  await context.close();
  return { ...result, blocked: [...new Set(blocked)].sort(), errors };
}

async function main() {
  const job = JSON.parse(readFileSync(process.argv[2], 'utf8'));
  const launch = () => chromium.launch(process.env.CHROME
    ? { executablePath: process.env.CHROME, headless: true }
    : { channel: 'chrome', headless: true });
  const queue = job.pairs.flatMap((pair) => job.viewports.map((viewport) => ({ pair, viewport })));
  // A browser per worker: one Chrome serializes its pages' screenshots.
  const browsers = await Promise.all(Array.from({ length: job.jobs || 4 }, launch));
  const worker = async (browser) => {
    const scratch = await (await browser.newContext()).newPage();
    await scratch.evaluate(`window.compare = ${compare.toString()}`);
    for (let task; (task = queue.shift());) {
      const { pair, viewport } = task;
      const base = join(job.out, `${pair.id}-${viewport.name}`);
      try {
        const ruby = await render(browser, pair.ruby, viewport, `${base}-ruby.png`);
        const adoctra = await render(browser, pair.adoctra, viewport, `${base}-adoctra.png`);
        const png = (side) => 'data:image/png;base64,' + readFileSync(`${base}-${side}.png`).toString('base64');
        const { image, ...pixels } = await scratch.evaluate(
          ([a, b, t]) => compare(a, b, t), [png('ruby'), png('adoctra'), job.threshold]);
        writeFileSync(`${base}-diff.png`, Buffer.from(image.split(',')[1], 'base64'));
        process.stdout.write(JSON.stringify({ id: pair.id, viewport: viewport.name, ruby, adoctra, pixels }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ id: pair.id, viewport: viewport.name, error: String(error) }) + '\n');
      }
    }
  };
  await Promise.all(browsers.map(worker));
  process.stdout.write(JSON.stringify({ browser: browsers[0].version() }) + '\n');
  await Promise.all(browsers.map((browser) => browser.close()));
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
