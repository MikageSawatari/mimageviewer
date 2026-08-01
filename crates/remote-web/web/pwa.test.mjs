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
  assert.match(css, /\.viewer-ui[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.viewer-ui\.top[\s\S]*safe-area-inset-top/);
  assert.match(css, /\.viewer-ui\.bottom[\s\S]*safe-area-inset-bottom/);
  assert.match(css, /\.viewer-seek[\s\S]*safe-area-inset-left[\s\S]*safe-area-inset-right/);
  assert.match(css, /\.command-menu[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-bottom[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.virtual-window[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
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
