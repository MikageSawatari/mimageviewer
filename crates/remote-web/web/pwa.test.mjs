import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const here = new URL("./", import.meta.url);

test("manifest describes the standalone shell and every supplied icon", async () => {
  const manifest = JSON.parse(
    await readFile(new URL("manifest.webmanifest", here), "utf8")
  );
  assert.equal(manifest.name, "mIV Remote");
  assert.ok(manifest.short_name);
  assert.equal(manifest.display, "standalone");
  assert.equal(manifest.start_url, "./");
  assert.equal(manifest.scope, "./");
  assert.equal(manifest.background_color, "#111318");
  assert.equal(manifest.theme_color, "#111318");

  const expected = new Map([
    ["icons/icon-192.png", [192, "any"]],
    ["icons/icon-512.png", [512, "any"]],
    ["icons/maskable-192.png", [192, "maskable"]],
    ["icons/maskable-512.png", [512, "maskable"]],
  ]);
  assert.equal(manifest.icons.length, expected.size);
  for (const icon of manifest.icons) {
    const [size, purpose] = expected.get(icon.src) ?? [];
    assert.ok(size, `unexpected manifest icon: ${icon.src}`);
    assert.equal(icon.sizes, `${size}x${size}`);
    assert.equal(icon.type, "image/png");
    assert.equal(icon.purpose, purpose);
    assert.deepEqual(await pngDimensions(icon.src), [size, size]);
  }
  assert.deepEqual(await pngDimensions("icons/icon-180.png"), [180, 180]);
});

test("HTML advertises the PWA and iOS standalone metadata", async () => {
  const html = await readFile(new URL("index.html", here), "utf8");
  assert.match(html, /name="viewport"[^>]*viewport-fit=cover/);
  assert.match(html, /name="apple-mobile-web-app-capable" content="yes"/);
  assert.match(
    html,
    /name="apple-mobile-web-app-status-bar-style" content="black-translucent"/
  );
  assert.match(html, /rel="manifest" href="\/manifest\.webmanifest"/);
  assert.match(
    html,
    /rel="apple-touch-icon" sizes="180x180" href="\/icons\/icon-180\.png"/
  );
});

test("the online-only remote shell does not register a service worker", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.doesNotMatch(app, /navigator\.serviceWorker|serviceWorker\.register/);
});

test("safe areas protect portrait bars and landscape side controls", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(css, /\.topbar[\s\S]*safe-area-inset-top[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.page-content[\s\S]*safe-area-inset-top[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-bottom[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.viewer-ui[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.viewer-ui\.top[\s\S]*safe-area-inset-top/);
  assert.match(css, /\.viewer-ui\.bottom[\s\S]*safe-area-inset-bottom/);
  assert.match(css, /\.viewer-seek[\s\S]*safe-area-inset-left[\s\S]*safe-area-inset-right/);
  assert.match(css, /\.command-menu[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-bottom[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.virtual-window[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(app, /--grid-inline-inset/);
  assert.doesNotMatch(app, /windowElement\.style\.(?:left|right)\s*=/);
  assert.doesNotMatch(css, /\.screen > \.topbar > :not\(\.menu-trigger\)/);
});

test("image tiles preserve portrait and landscape shape below a separate label row", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  // A percentage height in the preview's implicit auto grid row resolves to auto,
  // so the image must be taken out of that row and pinned to the preview box.
  assert.match(
    css,
    /\.tile-preview img\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0;[^}]*width:\s*100%;[^}]*height:\s*100%;/
  );
  assert.match(
    css,
    /\.folder-glyph,\s*\.file-glyph\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0;/
  );
  assert.match(css, /\.tile-preview img\s*\{[^}]*object-fit:\s*contain/);
  assert.doesNotMatch(css, /\.tile-preview img\s*\{[^}]*object-fit:\s*cover/);
  assert.match(css, /\.grid-tile\.image-tile \.tile-preview img\s*\{[^}]*object-fit:\s*contain/);
  assert.match(css, /\.grid-tile\s*\{[^}]*grid-template-rows:\s*var\(--grid-preview-height\) var\(--grid-label-height\)/);
  assert.match(css, /\.grid-cursor-visible \.grid-tile\.grid-active::after\s*\{[^}]*inset 0 0 0 3px var\(--accent\)/);
  assert.match(css, /\.grid-tile:focus-visible::after\s*\{[^}]*inset 0 0 0 3px var\(--accent\)/);
  assert.match(app, /tile\.append\(preview, label\)/);
  assert.match(app, /label\.append\([\s\S]*entry-detail-badge/);
  assert.doesNotMatch(css, /\.entry-detail-badge\s*\{[^}]*position:\s*absolute/);
});

test("the grid owns pinch scaling while one-finger vertical scroll stays native", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(css, /\.grid-scroll\s*\{[^}]*touch-action:\s*pan-y/);
  assert.match(
    app,
    /this\.onTouchMove = \(event\) => \{\s*if \(event\.touches\.length < 2\) return;\s*event\.preventDefault\(\);\s*\}/
  );
  assert.match(
    app,
    /addEventListener\("touchmove", this\.onTouchMove, \{\s*passive: false,?\s*\}\)/
  );
  assert.match(
    app,
    /removeEventListener\("touchmove", this\.onTouchMove\)/
  );
  assert.doesNotMatch(app, /縦持ちの列数|横持ちの列数/);
});

async function pngDimensions(relativePath) {
  const bytes = await readFile(new URL(relativePath, here));
  assert.deepEqual(
    [...bytes.subarray(0, 8)],
    [137, 80, 78, 71, 13, 10, 26, 10],
    `${relativePath} must be a PNG`
  );
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}
