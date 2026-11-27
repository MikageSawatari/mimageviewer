// Run: node --experimental-vm-modules scripts/generate-sound-effects.mjs
// Reads only the bundled distribution, never a user's mIV Sound installation.
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const vendor = path.join(root, 'vendor/miv-sound');
const web = path.join(vendor, 'mIV Sound.vst3/Contents/Resources/webview');
const output = path.join(root, 'src/sound_effects.generated.json');
const inputs = new Map();
const deferredIssues = [];
const deniedImports = new Set();
const unhandledErrors = [];
const onUnhandled = error => { if (!deniedImports.has(error)) unhandledErrors.push(error); };
process.on('unhandledRejection', onUnhandled);
function denyDynamicImport(relative, specifier) {
    const error = new Error(`Inert UI does not load dynamic module: ${specifier}`);
    deferredIssues.push({ relative, message: error.message, stack: error.stack });
    deniedImports.add(error);
    return Promise.reject(error);
}
function read(file) {
    const bytes = fs.readFileSync(file);
    inputs.set(path.relative(root, file).split(path.sep).join('/'), createHash('sha256').update(bytes).digest('hex'));
    return bytes.toString('utf8').replace(/^\uFEFF/, '');
}

// A deliberately inert DOM: no timers, audio host, fetch, file picker, or real
// browser. UI helpers are recorded by upstream effect-describe. A partial UI
// build is explicitly marked; defaults are captured before building any UI.
function element(tag = 'div', nodeType = 1) {
    const attributes = new Map();
    const children = [];
    const classNames = () => String(node.className || '').split(/\s+/).filter(Boolean);
    const canvasContext = new Proxy({
        measureText: text => ({ width: String(text).length * 8 }),
        createLinearGradient: () => ({ addColorStop() {} }),
        createRadialGradient: () => ({ addColorStop() {} }),
        createImageData(width, height) {
            if (!Number.isInteger(width) || !Number.isInteger(height) || width < 0 || height < 0 || width * height > 4 * 1024 * 1024) {
                throw new Error('Unsupported inert canvas image dimensions');
            }
            return { width, height, data: new Uint8ClampedArray(width * height * 4) };
        }
    }, { get(target, key) { return target[key] ?? (() => {}); } });
    const node = {
        tagName: String(tag).toUpperCase(), nodeType, parentNode: null, className: '',
        style: { setProperty() {}, removeProperty() {} }, dataset: {},
        get childNodes() { return children; },
        get children() { return children.filter(child => child.nodeType === 1); },
        get firstChild() { return children[0] || null; },
        get lastChild() { return children.at(-1) || null; },
        get firstElementChild() { return this.children[0] || null; },
        get lastElementChild() { return this.children.at(-1) || null; },
        get parentElement() { return this.parentNode?.nodeType === 1 ? this.parentNode : null; },
        get nextSibling() { const siblings = this.parentNode?.childNodes; return siblings?.[siblings.indexOf(this) + 1] || null; },
        get nextElementSibling() { let next = this.nextSibling; while (next && next.nodeType !== 1) next = next.nextSibling; return next; },
        classList: {
            add(...names) { node.className = [...new Set([...classNames(), ...names])].join(' '); },
            remove(...names) { node.className = classNames().filter(name => !names.includes(name)).join(' '); },
            contains(name) { return classNames().includes(name); },
            toggle(name, force) { const add = force ?? !this.contains(name); if (add) this.add(name); else this.remove(name); return add; }
        },
        appendChild(child) {
            if (!child || typeof child !== 'object') throw new Error('Invalid inert DOM child');
            if (child.nodeType === 11) { for (const entry of [...child.childNodes]) this.appendChild(entry); return child; }
            child.remove(); children.push(child); child.parentNode = this; return child;
        },
        append(...nodes) { for (const child of nodes) this.appendChild(typeof child === 'string' ? textNode(child) : child); },
        removeChild(child) {
            const index = children.indexOf(child);
            if (index < 0) throw new Error('Inert DOM node is not a child');
            children.splice(index, 1); child.parentNode = null; return child;
        },
        replaceChildren(...nodes) { for (const child of [...children]) this.removeChild(child); this.append(...nodes); },
        insertBefore(child, reference) {
            if (reference === null) return this.appendChild(child);
            if (!children.includes(reference)) throw new Error('Inert DOM reference is not a child');
            child.remove(); children.splice(children.indexOf(reference), 0, child); child.parentNode = this; return child;
        },
        setAttribute(name, value) {
            value = String(value); attributes.set(name, value);
            if (name === 'class') this.className = value;
            else if (name.startsWith('data-')) this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
            else if (['id', 'type', 'name', 'value'].includes(name)) this[name] = value;
        },
        getAttribute(name) {
            if (name === 'class') return this.className || null;
            if (name.startsWith('data-')) return this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] ?? null;
            return attributes.get(name) ?? (['id', 'type', 'name', 'value'].includes(name) ? this[name] ?? null : null);
        },
        removeAttribute(name) { attributes.delete(name); },
        addEventListener() {}, removeEventListener() {}, remove() { this.parentNode?.removeChild(this); },
        matches(selector) {
            // There is no user input or focused control in the generator.
            if (selector === ':active' || selector === ':focus' || selector === ':focus-visible') return false;
            const checked = selector.endsWith(':checked');
            if (checked) selector = selector.slice(0, -8);
            const attrs = [...selector.matchAll(/\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g)];
            const base = selector.replace(/\[[^\]]*\]/g, '');
            if (!/^(?:[\w*-]+)?(?:[.#][\w-]+)*$/.test(base)) throw new Error(`Unsupported inert selector: ${selector}`);
            const tag = /^[\w*-]+/.exec(base)?.[0];
            if (tag && tag !== '*' && this.tagName !== tag.toUpperCase()) return false;
            for (const match of base.matchAll(/([.#])([\w-]+)/g)) {
                if (match[1] === '.' ? !this.classList.contains(match[2]) : this.id !== match[2]) return false;
            }
            return (!checked || this.checked) && attrs.every(([, name, value]) => value === undefined
                ? this.getAttribute(name) !== null : this.getAttribute(name) === value);
        },
        closest(selector) { for (let current = this; current; current = current.parentElement) if (current.matches(selector)) return current; return null; },
        querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
        querySelectorAll(selector) {
            const alternatives = selector.split(',').map(part => part.trim().split(/\s+/));
            const matches = candidate => alternatives.some(parts => {
                if (!candidate.matches(parts.at(-1))) return false;
                let parent = candidate.parentElement;
                for (let index = parts.length - 2; index >= 0; index--) {
                    while (parent && !parent.matches(parts[index])) parent = parent.parentElement;
                    if (!parent) return false;
                    parent = parent.parentElement;
                }
                return true;
            });
            const found = [];
            const visit = parent => { for (const child of parent.children) { if (matches(child)) found.push(child); visit(child); } };
            visit(this); return found;
        },
        getBoundingClientRect() { return { width: 800, height: 400, left: 0, top: 0 }; },
        getContext() { return canvasContext; },
        clientWidth: 800, clientHeight: 400, offsetWidth: 800, offsetHeight: 400,
        isConnected: false, value: '', checked: false
    };
    let html = '', text = '', value = '';
    Object.defineProperties(node, {
        innerHTML: { get: () => html, set(content) { html = String(content); node.replaceChildren(); } },
        textContent: { get: () => text, set(content) { text = String(content); node.replaceChildren(); } },
        value: { get: () => value, set(content) { value = String(content); } }
    });
    return node;
}
function textNode(text) { const node = element('#text', 3); node.textContent = text; return node; }
const observer = class { observe() {} disconnect() {} unobserve() {} };
const sandbox = {
    console: { log() {}, warn() {}, error() {} },
    document: { createElement: element, createElementNS: (_, tag) => element(tag), createTextNode: textNode,
        createDocumentFragment: () => element('#fragment', 11),
        head: element(), body: element(), documentElement: element(),
        querySelector() { return element(); }, querySelectorAll() { return []; },
        getElementById() { return null; }, addEventListener() {}, removeEventListener() {} },
    MutationObserver: observer, ResizeObserver: observer, IntersectionObserver: observer,
    requestAnimationFrame() { return 0; }, cancelAnimationFrame() {},
    setTimeout() { return 0; }, clearTimeout() {}, setInterval() { return 0; }, clearInterval() {},
    performance: { now: () => 0 }, navigator: { userAgent: 'catalog-generator' },
    getComputedStyle: () => ({ getPropertyValue: () => '', color: '#000' }),
    devicePixelRatio: 1, addEventListener() {}, removeEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    localStorage: { getItem: () => null }, URL, TextEncoder, TextDecoder
};
sandbox.window = sandbox;
const context = vm.createContext(sandbox);
const modules = new Map();
async function moduleAt(file) {
    file = path.resolve(file);
    if (!file.startsWith(web + path.sep)) throw new Error('Module outside bundled webview');
    if (modules.has(file)) return modules.get(file);
    const module = new vm.SourceTextModule(read(file), { context, identifier: file,
        importModuleDynamically: specifier => denyDynamicImport(path.relative(web, file).split(path.sep).join('/'), specifier) });
    modules.set(file, module);
    await module.link((specifier, parent) => moduleAt(path.resolve(path.dirname(parent.identifier), specifier)));
    return module;
}
async function loadModule(relative) {
    const module = await moduleAt(path.join(web, relative));
    await module.evaluate({ timeout: 10_000 });
    return module.namespace;
}
function classic(relative) {
    const file = path.join(web, relative);
    vm.runInContext(read(file), context, { filename: relative, timeout: 10_000,
        importModuleDynamically: specifier => denyDynamicImport(relative, specifier) });
}
const support = await loadModule('js/plugin-support-scripts.js');
await loadModule('js/ui/range-fill.js');
for (const relative of support.PLUGIN_SUPPORT_SCRIPTS) classic(relative);
// Keep the real helpers' DOM structure. Both our union recorder and upstream
// effect-describe wrap them, without replacing the supplied implementation.
vm.runInContext(`for (const name of ['createParameterControl', 'createLogarithmicParameterControl',
    'createSelectControl', 'createRadioGroup', 'createCheckboxControl']) {
    const original = PluginBase.prototype[name];
    PluginBase.prototype[name] = function(...args) {
        const index = name === 'createParameterControl' || name === 'createLogarithmicParameterControl' ? 7
            : name === 'createCheckboxControl' ? 3 : 4;
        const key = args[index];
        if (typeof key === 'string') (this.__catalogControls ??= new Map()).set(key, {label: args[0]});
        return original.apply(this, args);
    };
}`, context);
const { describeEffect } = await loadModule('js/remote/effect-describe.js');
const { DSP_AUTOMATION_CATALOG: dsp, DSP_PARAM_LAYOUTS: dspLayouts } = await loadModule('js/audio/dsp-params.generated.js');
const { PARAMETER_HINTS: hints } = await loadModule('js/remote/parameter-hints.js');
const excluded = new Set(['SoundFontPlayerPlugin', 'SpeakerDistortionCorrectionPlugin',
    'NetworkSendPlugin', 'NetworkReceivePlugin']);
const definitions = [];
let inPlugins = false;
for (const line of read(path.join(web, 'plugins/plugins.txt')).split(/\r?\n/)) {
    const value = line.trim();
    if (value.startsWith('[')) { inPlugins = value === '[plugins]'; continue; }
    if (!inPlugins || !value || value.startsWith('#')) continue;
    const match = /^([^:]+):\s*([^|]+)\|\s*([^|]+)\|\s*([^|]+)(?:\|.*)?$/.exec(value);
    if (!match) throw new Error(`Invalid plugins.txt entry: ${value}`);
    const [, relative, name, category, type] = match.map(v => v?.trim());
    if (!excluded.has(type)) definitions.push({ relative: `plugins/${relative}.js`, name, category, type });
}
for (const definition of definitions) classic(definition.relative);
const reserved = new Set(['nm', 'en', 'ib', 'ob', 'ch', 'type', 'id', 'name', 'enabled',
    'inputBus', 'outputBus', 'channel', 'parameters']);
function shape(value) {
    if (value === null) return { type: 'null' };
    if (Array.isArray(value)) return { type: 'array', elements: value.map(shape) };
    if (typeof value === 'object') return { type: 'object', members: Object.fromEntries(
        Object.keys(value).sort().map(key => [key, shape(value[key])])) };
    return { type: typeof value };
}
function external(type, key) {
    return (type === 'IRReverbPlugin' && key === 'ir') ||
        (['SFZNotePlayerPlugin', 'SoundFontPlayerPlugin'].includes(type) && key === 'sf') ||
        (type === 'RoomEqPlugin' && /^(ms|ms(?:[0-9]|1[0-5]))$/.test(key)) ||
        (type === 'SpeakerDistortionCorrectionPlugin' && key === 'ms') ||
        (type === 'CrosstalkCancellationPlugin' && ['ll', 'lr', 'rl', 'rr'].includes(key));
}
const effects = [];
for (const definition of definitions.sort((a, b) => a.type < b.type ? -1 : a.type > b.type ? 1 : 0)) {
    const Plugin = sandbox[definition.type];
    if (typeof Plugin !== 'function') throw new Error(`Missing class ${definition.type}`);
    const create = () => {
        function Detached() {}
        Detached.prototype = Object.create(Plugin.prototype);
        Detached.prototype.audioHostActive = false;
        const plugin = Reflect.construct(Plugin, [], Detached);
        plugin.id = 1;
        return plugin;
    };
    const instance = create();
    const description = String(instance.description || '').split(/\r?\n/)[0];
    let describedInstance;
    const report = describeEffect({ createPlugin: () => {
        describedInstance = create();
        const build = describedInstance.createUI;
        describedInstance.createUI = function (...args) {
            try { return build.apply(this, args); }
            catch (error) {
                this.__catalogUiError = String(error?.message || error);
                throw error;
            }
        };
        return describedInstance;
    } }, definition.name);
    const descriptors = dsp[definition.type] || [];
    const rawHints = hints[definition.type];
    const resolvedHints = typeof rawHints === 'function' ? rawHints(instance) : rawHints || {};
    // Union the serializable/UI keys and DSP keys; neither source alone is a
    // complete description of saved state (e.g. FiveBandFIRPEQ).
    const params = new Map(report.params.filter(p => !reserved.has(p.key) && !p.key.startsWith('_ui'))
        .map(p => [p.key, p]));
    const saved = new Set(params.keys());
    const dspKeys = new Set();
    for (const descriptor of descriptors) {
        let key = descriptor.containerKey || descriptor.key;
        if (!descriptor.containerKey && !saved.has(key) && saved.has(`${key}${descriptor.element}`)) {
            key = `${key}${descriptor.element}`;
        }
        if (!reserved.has(key) && !key.startsWith('_ui')) dspKeys.add(key);
    }
    const controls = describedInstance.__catalogControls || new Map();
    // These per-channel references are saved only after a measurement is
    // selected, so the empty default instance alone cannot enumerate them.
    const conditionalKeys = definition.type === 'RoomEqPlugin'
        ? Array.from({ length: 16 }, (_, index) => `ms${index}`) : [];
    for (const key of new Set([...Object.keys(resolvedHints), ...controls.keys(), ...dspKeys, ...conditionalKeys])) {
        if (reserved.has(key) || key.startsWith('_ui') || params.has(key)) continue;
        const descriptor = descriptors.find(d => (d.containerKey || d.key) === key);
        params.set(key, { key, ...(controls.get(key) || {}), ...(resolvedHints[key] || {}),
            ...(descriptor ? { label: descriptor.title, min: descriptor.minimum, max: descriptor.maximum,
                unit: descriptor.unit || '' } : {}) });
    }
    const parameters = [...params.values()].sort((a, b) => a.key.localeCompare(b.key, 'en')).map(param => {
        const { key, type, default: defaultValue, ...metadata } = param;
        const hasDefault = Object.hasOwn(param, 'default');
        const hint = resolvedHints[key] || {};
        const dynamic = typeof rawHints === 'function' || !!hint.minKey || !!hint.maxKey ||
            /sample.?rate|depend|clamp|at least|ascending|channel count|follows/i.test(
                JSON.stringify([hint.description, metadata.description]));
        return { key, kind: type || 'unknown', ...(hasDefault ? { default: defaultValue, storage_shape: shape(defaultValue) } : {}),
            external_data: external(definition.type, key), dynamic_range: dynamic,
            ...(dynamic ? { range_note: 'Range is indicative and may depend on other values or sample rate; mIV Sound normalizes it.' } : {}),
            ...metadata };
    });
    effects.push({ type: definition.type, name: definition.name, category: definition.category, description,
        dsp: Object.hasOwn(dspLayouts, definition.type),
        keys_incomplete: !!report.uiError || [...saved].some(k => !dspKeys.has(k)) ||
            [...dspKeys].some(k => !saved.has(k)) || parameters.some(p => !Object.hasOwn(p, 'default')),
        ui_partial: !!report.uiError,
        ...(report.uiError ? { ui_error: describedInstance.__catalogUiError || 'UI build did not complete' } : {}),
        parameters });
    instance.cleanup?.();
}
// UI creation can schedule an asset refresh in a promise. Never load its runtime
// or user's asset store; report the unsupported part after queued microtasks
// settle, including rejections that upstream does not itself handle.
await new Promise(setImmediate);
for (const issue of deferredIssues) {
    const definition = definitions.find(definition => definition.relative === issue.relative)
        || definitions.find(definition => issue.stack?.includes(definition.type));
    if (!definition) throw new Error(`Unattributed deferred catalog operation: ${issue.relative}`);
    const effect = effects.find(effect => effect.type === definition.type);
    effect.ui_partial = true;
    effect.keys_incomplete = true;
    effect.ui_error = [effect.ui_error, issue.message].filter(Boolean).join('; ');
}
for (const error of unhandledErrors) {
    const definition = definitions.find(definition => String(error?.stack || error).includes(definition.type));
    if (!definition) throw error;
    const effect = effects.find(effect => effect.type === definition.type);
    effect.ui_partial = true;
    effect.keys_incomplete = true;
    effect.ui_error = [effect.ui_error, String(error?.message || error)].filter(Boolean).join('; ');
}
process.removeListener('unhandledRejection', onUnhandled);
// Hash the entire JS input inventory, including optional lazy-loaded helpers.
// This deliberately invalidates the catalog even for changes not reached by the
// inert UI, and detects added/removed JS files when checking source freshness.
function inventory(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
        const file = path.join(directory, entry.name);
        if (entry.isSymbolicLink()) throw new Error('Symlinks are not catalog inputs');
        if (entry.isDirectory()) inventory(file);
        else if (entry.name.endsWith('.js')) read(file);
    }
}
inventory(web);
const version = read(path.join(vendor, 'VERSION')).trim();
read(fileURLToPath(import.meta.url));
const document = { schema_version: 1, sound_version: version, native_dsp_types: Object.keys(dspLayouts).sort(),
    inputs: [...inputs].sort(([a], [b]) => a.localeCompare(b, 'en')).map(([path, sha256]) => ({ path, sha256 })),
    range_note: 'Ranges are indicative. mIV Sound normalizes values; retrieve the state after its reload to see normalized values.',
    external_data_note: 'Choose external data on screen. Existing stages retain their selected data when kept by id.', effects };
const bytes = JSON.stringify(document, null, 2) + '\n';
if (process.argv.includes('--check')) {
    if (fs.readFileSync(output, 'utf8') !== bytes) throw new Error('Sound effects catalog is stale; regenerate it');
} else fs.writeFileSync(output, bytes, 'utf8');
console.log(`${Buffer.byteLength(bytes)} bytes; ${effects.length} effects; ${effects.filter(e => e.keys_incomplete).length} incomplete; ${effects.filter(e => e.ui_partial).length} partial UI`);
