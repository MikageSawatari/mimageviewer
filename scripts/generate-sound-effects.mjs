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
// The generated packers clamp in the saved representation, before Float32
// conversion. Match every numeric slot, including arrays and object members;
// packedOffset, rather than a guessed key name, joins it to its descriptor.
const packRanges = new Map();
const packKeyRanges = new Map();
const dspSource = read(path.join(web, 'js/audio/dsp-params.generated.js'));
for (const [, type, body] of dspSource.matchAll(/export function pack(\w+)Params\(params = \{\}\) \{([\s\S]*?)\n\}/g)) {
    const slots = new Map();
    const keys = new Map();
    for (const [, offset, expression] of body.matchAll(/packed\[(\d+)\] = ([^\n]+);/g)) {
        const min = /if \(value < ([-\d.e+]+)\) return/.exec(expression);
        const max = /if \(value > ([-\d.e+]+)\) return/.exec(expression);
        if (!!min !== !!max) throw new Error(`One-sided numeric pack clamp: ${type}[${offset}]`);
        if (min && max) {
            const range = { min: Number(min[1]), max: Number(max[1]) };
            slots.set(Number(offset), range);
            // Non-automatable public keys have pack slots but no descriptor
            // (Bass Management / Spatial Mapper arrays and derived FIR delay).
            const input = /const value = (.*?);/.exec(expression)?.[1] || '';
            for (const match of input.matchAll(/params\["(\w+)"\](?:\[(\d+)\](?:\?\.\["(\w+)"\])?)?/g)) {
                const [, key, element, member] = match;
                const keyPath = element === undefined ? key : `${key}[]${member ? `.${member}` : ''}`;
                if (!keys.has(keyPath)) keys.set(keyPath, []);
                keys.get(keyPath).push({ ...range, offset: Number(offset), element: element === undefined ? null : Number(element) });
            }
        }
    }
    packRanges.set(type, slots);
    packKeyRanges.set(type, keys);
}

// Actual UI conversions / increments. Paths use [] for each array element.
// These are descriptions, never conversions applied to an AI write.
const valueOverrides = {
    // plugins/eq/tilt_eq.js:252-255 (hand-built slider edits ln Hz).
    'TiltEQPlugin.f0': { step: 0.01, display: { unit: 'Hz', min: Math.exp(3), max: Math.exp(9.9), formula: 'exp(v)' },
        evidence: { file: 'plugins/eq/tilt_eq.js', occurrences: [1, 1, 1, 3, 1], code: ['pivotLogSlider.min = 3.0;', 'pivotLogSlider.max = 9.9;',
            'pivotLogSlider.step = 0.01;', 'pivotHzInput.value = Math.round(Math.exp(this.f0));',
            'const clampedLog = Math.max(3.0, Math.min(9.9, logValue));'] } },
    // plugins/lofi/simple_jitter.js:167-169 (hand-built slider edits dB).
    'SimpleJitterPlugin.rj': { step: 1, display: { unit: 'ns', min: 0.001, max: 10000000, formula: '0.001*10^(v/20)' },
        evidence: { file: 'plugins/lofi/simple_jitter.js', occurrences: [1, 1, 1, 2, 1, 3], code: ["jitterSlider.min = '0';", "jitterSlider.max = '200';",
            "jitterSlider.step = '1';", 'const MIN_JITTER_NS = 0.001;',
            'const rmsJitterNs = MIN_JITTER_NS * Math.pow(10, this.rj / 20);', 'jitterValue.textContent = this.getDisplayValue();'] } },
    // plugins/analyzer/spectrum_analyzer.js:581-598 (exponent <-> point count).
    'SpectrumAnalyzerPlugin.pt': { display: { unit: 'points', min: 256, max: 16384, formula: '2^v' },
        evidence: { file: 'plugins/analyzer/spectrum_analyzer.js', occurrences: [3, 1, 1, 1], code: ['pointsValue.value = 1 << this.pt;',
            'pointsValue.min = 1 << 8;', 'pointsValue.max = 1 << 14;', 'const exponent = Math.round(Math.log2(numFFTPoints));'] } },
    // plugins/analyzer/spectrogram.js:751-766 (exponent <-> point count).
    'SpectrogramPlugin.pt': { display: { unit: 'points', min: 256, max: 16384, formula: '2^v' },
        evidence: { file: 'plugins/analyzer/spectrogram.js', occurrences: [3, 1, 1, 1], code: ['pointsValue.value = 1 << this.pt;',
            'pointsValue.min = 1 << 8;', 'pointsValue.max = 1 << 14;', 'const exponent = Math.round(Math.log2(numFFTPoints));'] } },
    // plugins/basics/stereo_balance.js:49-51,78-81 (saved ratio, percent widget).
    'StereoBalancePlugin.bl': { min: -1, max: 1, step: 0.01, unit: '', display: { unit: '%', min: -100, max: 100, formula: '100*v' },
        evidence: { file: 'plugins/basics/stereo_balance.js', occurrences: [1, 1, 1, 1], code: ["'Balance', -100, 100, 1, this.bl * 100,",
            '(value) => this.setBl(value / 100)', "'bl', (value) => value * 100",
            'this.bl = params.bl < -1 ? -1 : (params.bl > 1 ? 1 : params.bl);'] } },
    // plugins/analyzer/oscilloscope.js:331-340,611 (seconds saved, ms widget).
    'OscilloscopePlugin.dt': { unit: 's', display: { unit: 'ms', min: 1, max: 10000, formula: '1000*v' },
        evidence: { file: 'plugins/analyzer/oscilloscope.js', occurrences: [1, 1, 1, 1], code: ["'Display Time', 1, 10000, 1,",
            '(this.displayTime * 1000).toFixed(0)', 'this.setDisplayTime(value / 1000);',
            "'ms', 'displayTime', (value) => value * 1000, true"] } },
    // plugins/analyzer/oscilloscope.js:443-451,611 (seconds saved, ms widget).
    'OscilloscopePlugin.ho': { unit: 's', display: { unit: 'ms', min: 0.1, max: 10, formula: '1000*v' },
        evidence: { file: 'plugins/analyzer/oscilloscope.js', occurrences: [1, 1, 1, 1], code: ["'Holdoff', 0.1, 10, 0.1,",
            '(this.holdoff * 1000).toFixed(1)', 'this.setHoldoff(value / 1000);',
            "'ms', 'holdoff', (value) => value * 1000, true"] } },
    // plugins/analyzer/stereo_meter.js:228-233,273-276 (seconds -> ms).
    'StereoMeterPlugin.wt': { unit: 's', display: { unit: 'ms', min: 10, max: 1000, formula: '1000*v' },
        evidence: { file: 'plugins/analyzer/stereo_meter.js', occurrences: [1, 1, 1, 1], code: ["'Window', 10, 1000, 1,",
            '(this.windowTime * 1000).toFixed(0)', '(value) => this.setWindowTime(value / 1000)',
            "'windowTime', (value) => value * 1000, true"] } },
    // plugins/resonator/modal_resonator.js:358,546-583 (ln Hz -> Hz input).
    'ModalResonatorPlugin.rs[].fr': { unit: 'ln(Hz)', display: { unit: 'Hz', min: Math.exp(3), max: Math.exp(9.9), formula: 'exp(v)' },
        evidence: { file: 'plugins/resonator/modal_resonator.js', occurrences: [1, 3, 1, 1, 1], code: ["'Freq (Hz):', 3.00, 9.90, 0.01, resonator.fr, 'fr', i",
            "if (param === 'fr' || param === 'lp' || param === 'hp') {", 'input.value = Math.round(Math.exp(value));',
            'paramVal = Math.log(inputVal);', 'this.rs[index].fr = Math.max(3.00, Math.min(9.90, fr));'] } },
    // plugins/resonator/modal_resonator.js:370,546-583 (ln Hz -> Hz input).
    'ModalResonatorPlugin.rs[].lp': { unit: 'ln(Hz)', display: { unit: 'Hz', min: Math.exp(3), max: Math.exp(9.9), formula: 'exp(v)' },
        evidence: { file: 'plugins/resonator/modal_resonator.js', occurrences: [1, 3, 1, 1, 1], code: ["'LPF Freq (Hz):', 3.00, 9.90, 0.01, resonator.lp, 'lp', i",
            "if (param === 'fr' || param === 'lp' || param === 'hp') {", 'input.value = Math.round(Math.exp(value));',
            'paramVal = Math.log(inputVal);', 'this.rs[index].lp = Math.max(3.00, Math.min(9.90, lp));'] } },
    // plugins/resonator/modal_resonator.js:376,546-583 (ln Hz -> Hz input).
    'ModalResonatorPlugin.rs[].hp': { unit: 'ln(Hz)', display: { unit: 'Hz', min: Math.exp(3), max: Math.exp(9.9), formula: 'exp(v)' },
        evidence: { file: 'plugins/resonator/modal_resonator.js', occurrences: [1, 3, 1, 1, 1], code: ["'HPF Freq (Hz):', 3.00, 9.90, 0.01, resonator.hp, 'hp', i",
            "if (param === 'fr' || param === 'lp' || param === 'hp') {", 'input.value = Math.round(Math.exp(value));',
            'paramVal = Math.log(inputVal);', 'this.rs[index].hp = Math.max(3.00, Math.min(9.90, hp));'] } },
};
const usedOverrides = new Set();

// Content fingerprints tolerate line moves / whitespace, but never changes to
// the recorded conversion or clamp expressions. Validate before either writing
// or checking a catalog; input digests alone cannot reject stale overrides.
const fingerprintCode = code => code.replace(/\s+/g, '');
function verifyOverrideEvidence(key, override, source) {
    const evidence = override.evidence;
    if (!evidence?.file || !evidence.code?.length || evidence.occurrences?.length !== evidence.code.length) {
        throw new Error(`Missing override evidence: ${key}`);
    }
    const fingerprint = fingerprintCode(source);
    for (const [index, code] of evidence.code.entries()) {
        const expected = evidence.occurrences[index];
        if (!code || !Number.isSafeInteger(expected) || expected < 1 ||
            fingerprint.split(fingerprintCode(code)).length - 1 !== expected) {
            throw new Error(`Override evidence changed: ${key} (${evidence.file}); review the override before regenerating`);
        }
    }
}
for (const [key, override] of Object.entries(valueOverrides)) {
    verifyOverrideEvidence(key, override, read(path.join(web, override.evidence.file)));
}

function storedDescriptor(descriptor) {
    const d = descriptor;
    const transform = {
        identity: [v => v, d.unit || ''],
        naturalLog: [Math.log, `ln(${d.unit || 'value'})`],
        log10: [Math.log10, 'log10(probability)'],
        decibelsFromReference: [v => 20 * Math.log10(v / d.transformReference), `dB re ${d.transformReference} ${d.unit}`]
    }[d.transform];
    if (!transform) throw new Error(`Unsupported descriptor transform: ${d.transform}`);
    const [convert, unit] = transform;
    return { min: convert(d.minimum), max: convert(d.maximum), unit };
}

// Recognize only literal, direct JS clamp idioms. More complex/dynamic clamps
// retain their indicative UI/hint range unless the packer supplies evidence.
function jsRange(source, key) {
    const index = /\d+$/.exec(key)?.[0];
    if (index !== undefined) source = source.replace(/\$\{(?:index|band)\}/g, index)
        .replace(/\[`(\w+)`\]/g, '.$1')
        .replace(/\['(\w+)' \+ index\]/g, (_, prefix) => `.${prefix}${index}`);
    // A frozen range table used by the subclass's own setter (Headphone EQ).
    for (const [, table] of source.matchAll(/Object\.entries\((\w+_RANGES)\)/g)) {
        const range = vm.runInContext(`typeof ${table} === 'undefined' ? null : ${table}[${JSON.stringify(key)}]`, context);
        if (Array.isArray(range) && range.length === 2 && range.every(Number.isFinite)) return { min: range[0], max: range[1] };
    }
    const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const number = '(-?\\d+(?:\\.\\d*)?(?:e[+-]?\\d+)?)';
    const patterns = [
        `(?:this\\.\\w+\\s*=\\s*)?(?:this\\.)?parseFiniteNumber\\(\\s*params\\.${escaped},\\s*${number},\\s*${number}`,
        `this\\.${escaped}\\s*=\\s*Math\\.max\\(\\s*${number},\\s*Math\\.min\\(\\s*${number},`,
        `this\\.${escaped}\\s*=\\s*params\\.${escaped}\\s*<\\s*${number}\\s*\\?[^:]+:\\s*\\(params\\.${escaped}\\s*>\\s*${number}\\s*\\?`
    ];
    for (const pattern of patterns) {
        const match = new RegExp(pattern, 'i').exec(source);
        if (match) return { min: Number(match[1]), max: Number(match[2]) };
    }
    // The same helper also takes named constants / class range constants.
    // Resolve only identifier paths, never arbitrary expressions from source.
    const named = new RegExp(`parseFiniteNumber\\(\\s*params\\.${escaped},\\s*([\\w.]+),\\s*([\\w.]+)`).exec(source);
    if (named) {
        const values = named.slice(1).map(token => {
            if (Number.isFinite(Number(token))) return Number(token);
            if (!/^[A-Za-z_]\w*(?:\.\w+)*$/.test(token)) return undefined;
            try { return vm.runInContext(token, context, { timeout: 1000 }); } catch { return undefined; }
        });
        if (values.every(Number.isFinite)) return { min: values[0], max: values[1] };
    }
    const spread = new RegExp(`parseFiniteNumber\\(\\s*params\\.${escaped},\\s*\\.\\.\\.([A-Z_]+)`).exec(source);
    if (spread) {
        const range = vm.runInContext(spread[1], context, { timeout: 1000 });
        if (Array.isArray(range) && range.length === 2 && range.every(Number.isFinite)) return { min: range[0], max: range[1] };
    }
    // Serialization aliases such as dt: this.displayTime still refer to the
    // same saved value. Require a direct property assignment and a ternary
    // returning exactly the bounds it compares against.
    const property = new RegExp(`\\b${escaped}:\\s*this\\.(\\w+)`).exec(source)?.[1] || key;
    const ternary = new RegExp(`this\\.${property}\\s*=\\s*(\\w+)\\s*<\\s*${number}\\s*\\?\\s*${number}\\s*:\\s*\\(\\1\\s*>\\s*${number}\\s*\\?\\s*${number}\\s*:\\s*\\1\\)`, 'i').exec(source);
    if (ternary && Number(ternary[2]) === Number(ternary[3]) && Number(ternary[4]) === Number(ternary[5])) {
        return { min: Number(ternary[2]), max: Number(ternary[4]) };
    }
    return null;
}

// Bounds whose setters use dynamic limits or indirect/indexed inputs. The
// default-context dynamic limits come from the upstream hint that calls the
// same _delayLimitMs(); they remain explicitly marked dynamic_range.
function indirectJsRange(type, key, metadata) {
    // plugins/analyzer/spectrum_analyzer.js:191-194;
    // plugins/analyzer/spectrogram.js:285-287 (indirect setPoints helper).
    if (['SpectrumAnalyzerPlugin', 'SpectrogramPlugin'].includes(type) && key === 'pt') return { min: 8, max: 14 };
    // plugins/others/sfz_note_player.js:51-57 (local keyed range table).
    if (type === 'SFZNotePlayerPlugin' && ['mn', 'mx', 'pl'].includes(key)) {
        return { min: key === 'pl' ? 1 : 21, max: key === 'pl' ? 128 : 108 };
    }
    // plugins/spatial/phase_select_eq.js:105-141: core intervals have a
    // minimum width of 1; the outer bounds must enclose their core interval.
    if (type === 'PhaseSelectEqPlugin' && key.startsWith('regions[].')) {
        const range = {
            ofl: [20, 39999], fl: [20, 39999], fh: [21, 40000], ofh: [21, 40000],
            opl: [0, 179], pl: [0, 179], ph: [1, 180], oph: [1, 180],
            obl: [-100, 99], bl: [-100, 99], bh: [-99, 100], obh: [-99, 100]
        }[key.split('.').at(-1)];
        if (range) {
            metadata.dynamic_range = true;
            metadata.range_note = 'Outer bounds enclose the core; core frequency, phase and balance intervals have minimum widths of 1 Hz, 1 degree and 1 percent point.';
            return { min: range[0], max: range[1] };
        }
    }
    // plugins/eq/group_delay_eq.js:82-90,119-128;
    // plugins/eq/group_delay_peq.js:126-137,174-199.
    if (['GroupDelayEqPlugin', 'GroupDelayPEQPlugin'].includes(type) && /^d\d+$/.test(key)) {
        return { min: metadata.min, max: metadata.max };
    }
    // plugins/eq/group_delay_peq.js:34,192-196 (named Q_RANGE).
    if (type === 'GroupDelayPEQPlugin' && /^q\d+$/.test(key)) return { min: 0.1, max: 100 };
    // plugins/eq/headphone_eq.js:228-230;
    // plugins/eq/tonal_balance_eq.js:323-328 (shelves have a conditional max of 2).
    if (['HeadphoneEQPlugin', 'TonalBalanceEQPlugin'].includes(type) && /^qa\d+$/.test(key)) {
        metadata.dynamic_range = true;
        metadata.range_note = 'Q is limited to 2 for shelf filters and 10 for peaking filters.';
        return { min: 0.1, max: 10 };
    }
    // plugins/basics/fir_crossover.js:147-156 (ascending active crossovers).
    if (type === 'FIRCrossoverPlugin' && /^f[1-3]$/.test(key)) {
        metadata.dynamic_range = true;
        metadata.range_note = 'Active crossovers must ascend by at least 1 Hz; bounds also depend on the band count.';
        return { min: 10, max: 40000 };
    }
    // plugins/eq/room_eq.js:1197-1205,1434-1435 (rounded/manual lower bound).
    if (type === 'RoomEqPlugin' && key === 'pl') return { min: 20, max: 20000 };
    // plugins/eq/room_eq.js:1256-1261 (dl is milliseconds, dy0 is legacy samples).
    if (type === 'RoomEqPlugin' && key === 'dl') return { min: 0, max: 20 };
    // plugins/eq/room_eq.js:1374-1381 (validated objects, no native band pack).
    if (type === 'RoomEqPlugin') {
        return { 'bs[].frequency': { min: 20, max: 20000 },
            'bs[].gain': { min: -20, max: 20 }, 'bs[].q': { min: 0.1, max: 10 } }[key];
    }
    // plugins/dynamics/brickwall_limiter.js:734 (throws outside the range).
    if (type === 'BrickwallLimiterPlugin' && key === 'la') return { min: 0, max: 10 };
    return null;
}

const numericChecks = [];
function normalizeNumber(metadata, descriptors, type, keyPath, source, defaults = [], element = null) {
    const descriptor = descriptors[0];
    const original = { ...metadata };
    const saved = metadata.value;
    delete metadata.value;
    delete metadata.scaled;
    const transformed = descriptor && descriptor.transform !== 'identity';
    if (transformed) {
        Object.assign(metadata, storedDescriptor(descriptor));
        // A physical increment has no constant increment in logarithmic storage.
        // Keep a UI step only when that widget already edits the saved value.
        if (original.source !== 'ui' || original.scaled) delete metadata.step;
    } else if (saved) Object.assign(metadata, saved);
    const overrideKey = `${type}.${keyPath}`;
    if (Object.hasOwn(valueOverrides, overrideKey)) {
        const { evidence, ...values } = valueOverrides[overrideKey];
        Object.assign(metadata, values);
        usedOverrides.add(overrideKey);
    }
    const bounds = descriptors.map(d => packRanges.get(type)?.get(d.packedOffset)).filter(Boolean);
    const keyBounds = (packKeyRanges.get(type)?.get(keyPath) || []).filter(range => element === null || range.element === null || range.element === element);
    bounds.push(...keyBounds);
    const js = jsRange(source, keyPath) || indirectJsRange(type, keyPath, metadata);
    if (js) bounds.push(js);
    if (bounds.length) {
        metadata.min = Math.max(...bounds.map(range => range.min));
        metadata.max = Math.min(...bounds.map(range => range.max));
    }
    if (!bounds.length) metadata.range_verified = false;
    else delete metadata.range_verified;
    // These setters normalize related values together. Their independent
    // envelopes are still valid, but not every combination is retained.
    let dependency;
    // plugins/modulation/chorus.js:245.
    if (type === 'ChorusPlugin' && keyPath === 'dp') dependency = 'Depth cannot exceed delay (dl).';
    // plugins/modulation/frequency_shifter.js:437-439.
    if (type === 'FrequencyShifterPlugin' && ['mn', 'mx'].includes(keyPath)) dependency = 'Minimum and maximum shift are sorted together.';
    // plugins/others/sfz_note_player.js:66-67.
    if (type === 'SFZNotePlayerPlugin' && keyPath === 'mx') dependency = 'Highest note is at least mn.';
    if (type === 'SFZNotePlayerPlugin' && keyPath === 'vc') dependency = 'Velocity ceiling is at least vf + 1.';
    // plugins/analyzer/pitch_meter.js:123-133; note_spectrogram.js:404-418.
    if (['PitchMeterPlugin', 'NoteSpectrogramPlugin'].includes(type) && ['mn', 'mx'].includes(keyPath)) dependency = 'Lowest and highest notes are normalized together.';
    // plugins/analyzer/rhythm_analyzer.js:284-291.
    if (type === 'RhythmAnalyzerPlugin' && ['mn', 'mx'].includes(keyPath)) dependency = 'Maximum BPM is at least 1.25 times minimum BPM; changing one can adjust the other.';
    // plugins/dynamics/multiband_compressor.js:955-970;
    // multiband_expander.js:824-839; multiband_transient.js:607-614;
    // plugins/spatial/multiband_balance.js:470-488;
    // plugins/saturation/multiband_saturation.js:386-394,452-454.
    if (['MultibandCompressorPlugin', 'MultibandExpanderPlugin', 'MultibandTransientPlugin', 'MultibandBalancePlugin', 'MultibandSaturationPlugin'].includes(type) && /^f[2-4]$/.test(keyPath)) dependency = 'Crossover cannot be below the preceding crossover.';
    if (dependency) {
        metadata.dynamic_range = true;
        metadata.range_note = `${dependency} mIV Sound normalizes it.`;
    }
    // The scaled upstream `value` supersedes the display range, including 0
    // for Adaptive Prediction's Infinity setting. Its display depends on other
    // state, so it has no misleading single-variable display formula.
    numericChecks.push({ type, keyPath, metadata, bounds, defaults, descriptors, keyBounds });
}

function normalizeParameter(parameter, descriptors, type, source) {
    const firstCheck = numericChecks.length;
    const key = parameter.key;
    const direct = descriptors.filter(d => !d.containerKey && (d.key === key || `${d.key}${d.element}` === key));
    const contained = descriptors.filter(d => d.containerKey === key);
    const defaults = Array.isArray(parameter.default) ? parameter.default : [];
    if (parameter.kind === 'number' || (parameter.kind === 'unknown' &&
        (parameter.control === 'number' || direct.some(d => ['float', 'int'].includes(d.kind))))) {
        normalizeNumber(parameter, direct, type, key, source, [parameter.default]);
    } else if (Array.isArray(parameter.default) || parameter.items || parameter.elements || parameter.members) {
        if (contained.some(d => d.memberKey) || parameter.members) {
            parameter.members ??= {};
            const members = new Set([...Object.keys(parameter.members), ...defaults.flatMap(v => v && typeof v === 'object' ? Object.keys(v) : [])]);
            for (const member of members) {
                const values = defaults.map(value => value?.[member]).filter(value => typeof value === 'number');
                const ds = contained.filter(d => d.memberKey === member);
                if (values.length || ds.some(d => ['float', 'int'].includes(d.kind))) {
                    parameter.members[member] ??= {};
                    const meta = parameter.members[member];
                    if (Array.isArray(meta)) meta.forEach((entry, index) => normalizeNumber(entry, ds.filter(d => d.element === index), type, `${key}[].${member}`, source, [defaults[index]?.[member]], index));
                    else normalizeNumber(meta, ds, type, `${key}[].${member}`, source, values);
                }
            }
        } else if (defaults.some(v => typeof v === 'number') || parameter.items?.control === 'number' || parameter.elements) {
            const ds = contained.length ? contained : direct;
            if (parameter.elements) parameter.elements.forEach((entry, index) => {
                if (typeof defaults[index] === 'number' || ds.some(d => d.element === index && ['float', 'int'].includes(d.kind))) {
                    normalizeNumber(entry, ds.filter(d => d.element === index), type, `${key}[]`, source, [defaults[index]], index);
                }
            });
            else {
                parameter.items ??= {};
                normalizeNumber(parameter.items, ds, type, `${key}[]`, source, defaults.filter(v => typeof v === 'number'));
            }
        }
    }
    if (numericChecks.slice(firstCheck).some(check => check.metadata.dynamic_range)) {
        parameter.dynamic_range = true;
        parameter.range_note ??= 'Range depends on other parameter values; see member descriptions. mIV Sound normalizes it.';
    }
}

// Follow saved shapes rather than UI controls: HTML select values are strings
// even when the plugin serializes numbers. Shared array/member descriptions
// are visited against every saved element, including per-element metadata.
function visitSavedDescriptions(metadata, savedShape, value, keyPath, visit) {
    if (!savedShape) return;
    visit(metadata, savedShape, value, keyPath);
    const members = (descriptions, shapes, object, prefix, index = null) => {
        for (const [key, childShape] of Object.entries(shapes || {})) {
            const child = descriptions?.[key];
            if (!child) continue;
            visitSavedDescriptions(Array.isArray(child) && index !== null ? child[index] : child,
                childShape, object?.[key], `${prefix}.${key}`, visit);
        }
    };
    if (savedShape.type === 'array') {
        savedShape.elements.forEach((childShape, index) => {
            const child = metadata.elements?.[index] || metadata.items;
            if (child) visitSavedDescriptions(child, childShape, value[index], `${keyPath}[${index}]`, visit);
            else if (childShape.type === 'object') members(metadata.members, childShape.members, value[index], `${keyPath}[${index}]`, index);
        });
    } else if (savedShape.type === 'object') members(metadata.members, savedShape.members, value, keyPath);
}

function normalizeOptionValues(metadata, savedShape, _value, keyPath) {
    for (const option of metadata.options || []) {
        const value = option.value;
        if (savedShape.type === 'number' && typeof value === 'string' &&
            /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(value.trim())) option.value = Number(value);
        else if (savedShape.type === 'string' && ['number', 'boolean'].includes(typeof value)) option.value = String(value);
        else if (savedShape.type === 'boolean' && ['true', 'false'].includes(value)) option.value = value === 'true';
        if (shape(option.value).type !== savedShape.type ||
            (typeof option.value === 'number' && !Number.isFinite(option.value))) {
            throw new Error(`Option does not match saved ${savedShape.type}: ${keyPath}`);
        }
    }
}
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
    const source = read(path.join(web, definition.relative));
    for (const parameter of parameters) {
        normalizeParameter(parameter, descriptors, definition.type, source);
        visitSavedDescriptions(parameter, parameter.storage_shape, parameter.default,
            `${definition.type}.${parameter.key}`, normalizeOptionValues);
    }
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
    range_note: 'Values, units, ranges, steps and defaults use stored form. display gives the display unit, range and formula from stored v. scale: log describes knob motion, not stored form. range_verified: false marks indicative ranges without a located JS or DSP pack clamp. Ranges may depend on other values or sample rate; mIV Sound normalizes values. Retrieve state after reload.',
    external_data_note: 'Choose external data on screen. Existing stages retain their selected data when kept by id.', effects };
const bytes = JSON.stringify(document, null, 2) + '\n';
if (process.argv.includes('--check')) {
    for (const [key, override] of Object.entries(valueOverrides)) {
        if (!usedOverrides.has(key)) throw new Error(`Unused value override: ${key}`);
        const source = read(path.join(web, override.evidence.file));
        // A line move is harmless, but changing any recorded expression must
        // reject the override. Exercise this without modifying bundled files.
        verifyOverrideEvidence(key, override, `\n\n${source}`);
        for (const code of override.evidence.code) {
            const fingerprint = fingerprintCode(source);
            const expression = fingerprintCode(code);
            for (let offset = fingerprint.indexOf(expression); offset >= 0; offset = fingerprint.indexOf(expression, offset + expression.length)) {
                const changed = fingerprint.slice(0, offset) + '__changed_expression__' + fingerprint.slice(offset + expression.length);
                let rejected = false;
                try { verifyOverrideEvidence(key, override, changed); } catch (error) {
                    rejected = error.message.startsWith(`Override evidence changed: ${key}`);
                }
                if (!rejected) throw new Error(`Changed override evidence was accepted: ${key}`);
            }
        }
    }
    for (const effect of effects) {
        for (const parameter of effect.parameters) {
            if (!parameter.storage_shape) continue;
            if (parameter.kind !== parameter.storage_shape.type ||
                JSON.stringify(shape(parameter.default)) !== JSON.stringify(parameter.storage_shape)) {
                throw new Error(`Default / storage shape mismatch: ${effect.type}.${parameter.key}`);
            }
            visitSavedDescriptions(parameter, parameter.storage_shape, parameter.default,
                `${effect.type}.${parameter.key}`, (metadata, savedShape, value, keyPath) => {
                    if (shape(value).type !== savedShape.type || (metadata.options || []).some(option => shape(option.value).type !== savedShape.type)) {
                        throw new Error(`Default / shape / option type mismatch: ${keyPath}`);
                    }
                });
        }
    }
    for (const [type, key] of [['DigitalErrorEmulatorPlugin', 'be'], ['G726ADPCMSimulatorPlugin', 're']]) {
        if (effects.find(effect => effect.type === type).parameters.find(parameter => parameter.key === key).display) {
            throw new Error(`${type}.${key}: the screen shows the saved exponent, not a converted probability`);
        }
    }
    const coveredSlots = new Set();
    for (const { type, keyPath, metadata: m, bounds, defaults, descriptors, keyBounds } of numericChecks) {
        const fail = reason => { throw new Error(`${type}.${keyPath}: ${reason}`); };
        if (m.display && !valueOverrides[`${type}.${keyPath}`]?.display) fail('display has no actual UI evidence');
        const inside = (v, min, max) => Number.isFinite(v) &&
            (min === undefined || v >= min - 1e-9 * Math.max(1, Math.abs(min))) &&
            (max === undefined || v <= max + 1e-9 * Math.max(1, Math.abs(max)));
        if (m.min !== undefined && m.max !== undefined && m.min > m.max) fail('empty range');
        for (const range of bounds) {
            if (!inside(m.min, range.min, range.max) || !inside(m.max, range.min, range.max)) fail('range exceeds clamp');
        }
        for (const d of descriptors) {
            coveredSlots.add(`${type}:${d.packedOffset}`);
            const range = packRanges.get(type)?.get(d.packedOffset);
            if (range && !inside(d.packedDefault, range.min, range.max)) fail('packedDefault exceeds pack clamp');
            if (d.transform !== 'identity') {
                const savedDefault = { naturalLog: () => Math.log(d.default), log10: () => Math.log10(d.default),
                    decibelsFromReference: () => 20 * Math.log10(d.default / d.transformReference) }[d.transform]();
                if (Math.abs(savedDefault - d.packedDefault) > 1e-9 * Math.max(1, Math.abs(d.packedDefault))) {
                    fail('descriptor default and packedDefault disagree');
                }
            }
        }
        for (const range of keyBounds) coveredSlots.add(`${type}:${range.offset}`);
        for (const value of defaults.filter(v => typeof v === 'number')) {
            if (!inside(value, m.min, m.max)) fail(`default ${value} exceeds stored range`);
            if (m.display) {
                // Only the small, documented expression vocabulary is accepted.
                const formula = m.display.formula;
                let display;
                if (formula === 'exp(v)') display = Math.exp(value);
                else if (formula === '2^v') display = 2 ** value;
                else if (formula === '100*v') display = 100 * value;
                else if (formula === '1000*v') display = 1000 * value;
                else {
                    const match = /^([\d.]+)\*10\^\(v\/20\)$/.exec(formula);
                    if (!match) fail(`unsupported formula: ${formula}`);
                    display = Number(match[1]) * 10 ** (value / 20);
                }
                if (!inside(display, m.display.min, m.display.max)) fail('display default exceeds range');
            }
        }
    }
    for (const effect of effects) {
        // Packers also contain internal fields (for example resetToken) which
        // have no public descriptor and are not saved parameter keys.
        for (const { packedOffset: offset } of dsp[effect.type] || []) {
            if (!packRanges.get(effect.type)?.has(offset)) continue;
            if (!coveredSlots.has(`${effect.type}:${offset}`)) {
                throw new Error(`Numeric pack slot has no catalog range check: ${effect.type}[${offset}]`);
            }
        }
        for (const [keyPath, ranges] of packKeyRanges.get(effect.type) || []) {
            const parameter = effect.parameters.find(p => p.key === keyPath.split('[]')[0]);
            if (!parameter || (keyPath === parameter.key && parameter.kind !== 'number')) continue;
            for (const { offset } of ranges) {
                if (!coveredSlots.has(`${effect.type}:${offset}`)) {
                    throw new Error(`Public numeric pack key has no range check: ${effect.type}.${keyPath}[${offset}]`);
                }
            }
        }
    }
    if (fs.readFileSync(output, 'utf8') !== bytes) throw new Error('Sound effects catalog is stale; regenerate it');
} else fs.writeFileSync(output, bytes, 'utf8');
console.log(`${Buffer.byteLength(bytes)} bytes; ${effects.length} effects; ${effects.filter(e => e.keys_incomplete).length} incomplete; ${effects.filter(e => e.ui_partial).length} partial UI`);
