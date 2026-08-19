package dev.tempera.android.browser;

import org.json.JSONObject;

final class DomProgram {
    private static final int MAX_NODES = 600;
    private static final String GLOBAL = "__temperaAgentRuntimeV1";

    // Installed once per document. The resident runtime maintains a mutation-
    // driven semantic cache and stable per-element references so unchanged
    // snapshots avoid a full DOM/style scan.
    private static final String CORE = """
        const __tempera = (() => {
          const MAX_NODES = %d;
          const selector = [
            'a[href]', 'button', 'input', 'textarea', 'select', 'option',
            '[role]', '[tabindex]', '[contenteditable="true"]',
            'summary', 'label', 'video[controls]', 'audio[controls]'
          ].join(',');
          const refs = new WeakMap();
          const elementsByRef = new Map();
          let nextRef = 1;
          let dirty = true;
          let cached = null;
          let generation = 0;

          const markDirty = () => { dirty = true; generation += 1; };
          const normalize = value => String(value || '').replace(/\\s+/g, ' ').trim();
          const lower = value => normalize(value).toLowerCase();
          const visible = element => {
            if (!(element instanceof Element) || !element.isConnected) return false;
            const style = getComputedStyle(element);
            if (style.display === 'none' || style.visibility === 'hidden' || Number(style.opacity) === 0) return false;
            const rect = element.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0 && rect.bottom >= 0 && rect.right >= 0 &&
                   rect.top <= innerHeight && rect.left <= innerWidth;
          };
          const roleOf = element => {
            const explicit = normalize(element.getAttribute('role'));
            if (explicit) return explicit;
            const tag = element.tagName.toLowerCase();
            if (tag === 'a') return 'link';
            if (tag === 'button' || (tag === 'input' && ['button','submit','reset'].includes(lower(element.type)))) return 'button';
            if (tag === 'input') return lower(element.type) === 'checkbox' ? 'checkbox' : lower(element.type) === 'radio' ? 'radio' : 'textbox';
            if (tag === 'textarea' || element.isContentEditable) return 'textbox';
            if (tag === 'select') return 'combobox';
            if (tag === 'option') return 'option';
            if (tag === 'summary') return 'button';
            return tag;
          };
          const labelOf = element => {
            const aria = normalize(element.getAttribute('aria-label'));
            if (aria) return aria;
            const labelledBy = normalize(element.getAttribute('aria-labelledby'));
            if (labelledBy) {
              const joined = labelledBy.split(/\\s+/).map(id => document.getElementById(id)).filter(Boolean)
                .map(node => normalize(node.innerText || node.textContent)).filter(Boolean).join(' ');
              if (joined) return joined;
            }
            const placeholder = normalize(element.getAttribute('placeholder'));
            if (placeholder) return placeholder;
            const title = normalize(element.getAttribute('title'));
            if (title) return title;
            if (element.labels && element.labels.length) {
              const joined = Array.from(element.labels).map(node => normalize(node.innerText || node.textContent)).filter(Boolean).join(' ');
              if (joined) return joined;
            }
            const text = normalize(element.innerText || element.textContent);
            if (text) return text.slice(0, 240);
            const name = normalize(element.getAttribute('name'));
            if (name) return name;
            return normalize(element.id);
          };
          const isSensitive = element => {
            const type = lower(element.getAttribute('type'));
            const autocomplete = lower(element.getAttribute('autocomplete'));
            return type === 'password' || autocomplete.includes('cc-') || autocomplete.includes('one-time-code');
          };
          const fnv = value => {
            let hash = 0xcbf29ce484222325n;
            const prime = 0x100000001b3n;
            for (let index = 0; index < value.length; index += 1) {
              const code = value.charCodeAt(index);
              hash ^= BigInt(code & 0xff);
              hash = BigInt.asUintN(64, hash * prime);
              hash ^= BigInt((code >>> 8) & 0xff);
              hash = BigInt.asUintN(64, hash * prime);
            }
            return hash.toString(16).padStart(16, '0');
          };
          const referenceFor = element => {
            let ref = refs.get(element);
            if (!ref) {
              ref = '@d' + nextRef++;
              refs.set(element, ref);
            }
            return ref;
          };
          const semanticElements = () => Array.from(document.querySelectorAll(selector))
            .filter(visible)
            .slice(0, MAX_NODES);
          const captureFresh = () => {
            const raw = semanticElements();
            const activeRefs = new Set();
            const nodes = raw.map(element => {
              const ref = referenceFor(element);
              activeRefs.add(ref);
              elementsByRef.set(ref, element);
              const rect = element.getBoundingClientRect();
              const role = roleOf(element);
              const sensitive = isSensitive(element);
              const value = sensitive ? '' : normalize(element.value || '');
              return {
                ref,
                role,
                label: labelOf(element),
                value: value.slice(0, 240),
                disabled: Boolean(element.disabled) || element.getAttribute('aria-disabled') === 'true',
                checked: typeof element.checked === 'boolean' ? element.checked : undefined,
                selected: typeof element.selected === 'boolean' ? element.selected : undefined,
                sensitive,
                bounds: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)]
              };
            });
            for (const ref of elementsByRef.keys()) {
              if (!activeRefs.has(ref)) elementsByRef.delete(ref);
            }
            const canonical = JSON.stringify({
              url: location.href,
              title: document.title,
              viewport: [innerWidth, innerHeight, devicePixelRatio],
              nodes: nodes.map(node => [node.ref,node.role,node.label,node.value,node.disabled,node.checked,node.selected,node.sensitive,node.bounds])
            });
            cached = {
              schemaVersion: 'tempera.android.browser.snapshot/v1',
              url: location.href,
              title: document.title,
              documentStateHash: 'fnv1a64:' + fnv(canonical),
              viewport: {width: innerWidth, height: innerHeight, scale: devicePixelRatio},
              nodes,
              truncated: raw.length >= MAX_NODES,
              trustedForConsequentialActions: false,
              semanticGeneration: generation,
              semanticCacheHit: false
            };
            dirty = false;
            return cached;
          };
          const capture = () => {
            if (dirty || !cached) return captureFresh();
            return {...cached, semanticCacheHit: true};
          };
          const captureDelta = previousStateHash => {
            const snapshot = capture();
            if (previousStateHash && snapshot.documentStateHash === previousStateHash) {
              return {
                schemaVersion: 'tempera.android.browser.snapshot-delta/v1',
                unchanged: true,
                documentStateHash: snapshot.documentStateHash,
                url: snapshot.url,
                title: snapshot.title,
                semanticGeneration: snapshot.semanticGeneration,
                semanticCacheHit: snapshot.semanticCacheHit
              };
            }
            return {...snapshot, unchanged: false};
          };
          const setValue = (element, value) => {
            const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
            const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
            if (descriptor && descriptor.set) descriptor.set.call(element, value);
            else element.value = value;
            element.dispatchEvent(new InputEvent('input', {bubbles: true, inputType: 'insertText', data: value}));
            element.dispatchEvent(new Event('change', {bubbles: true}));
          };
          const resolve = ref => {
            const element = elementsByRef.get(String(ref || ''));
            return element && element.isConnected && visible(element) ? element : null;
          };
          const act = request => {
            const before = capture();
            if (!request.expectedStateHash || request.expectedStateHash !== before.documentStateHash) {
              return {ok: false, stale: true, error: 'document state changed', before};
            }
            const ref = String(request.ref || '');
            if (!/^@d[1-9][0-9]*$/.test(ref)) return {ok: false, stale: false, error: 'invalid or missing DOM ref', before};
            const element = resolve(ref);
            if (!element) return {ok: false, stale: true, error: 'DOM ref expired', before};
            const kind = String(request.kind || 'tap');
            markDirty();
            if (kind === 'tap' || kind === 'click') {
              element.focus({preventScroll: true});
              element.click();
            } else if (kind === 'fill') {
              if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) {
                return {ok: false, stale: false, error: 'fill target is not a text control', before};
              }
              setValue(element, String(request.text || ''));
            } else if (kind === 'type') {
              if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) {
                return {ok: false, stale: false, error: 'type target is not a text control', before};
              }
              setValue(element, String(element.value || '') + String(request.text || ''));
            } else if (kind === 'scrollIntoView') {
              element.scrollIntoView({block: 'center', inline: 'center', behavior: 'instant'});
            } else {
              return {ok: false, stale: false, error: 'unsupported DOM action: ' + kind, before};
            }
            const after = captureFresh();
            return {
              ok: true,
              stale: false,
              receipt: {
                schemaVersion: 'tempera.android.browser.action-receipt/v1',
                kind,
                ref,
                beforeStateHash: before.documentStateHash,
                afterStateHash: after.documentStateHash,
                sensitiveTarget: Boolean(before.nodes.find(node => node.ref === ref)?.sensitive)
              },
              after
            };
          };
          const scroll = request => {
            const before = capture();
            if (!request.expectedStateHash || request.expectedStateHash !== before.documentStateHash) {
              return {ok: false, stale: true, error: 'document state changed', before};
            }
            const direction = String(request.direction || 'down');
            const amount = Math.max(64, Math.round(innerHeight * 0.72));
            const dx = direction === 'left' ? -amount : direction === 'right' ? amount : 0;
            const dy = direction === 'up' ? -amount : direction === 'down' ? amount : 0;
            markDirty();
            scrollBy({left: dx, top: dy, behavior: 'instant'});
            const after = captureFresh();
            return {
              ok: true,
              stale: false,
              receipt: {
                schemaVersion: 'tempera.android.browser.action-receipt/v1',
                kind: 'scroll', direction,
                beforeStateHash: before.documentStateHash,
                afterStateHash: after.documentStateHash
              },
              after
            };
          };

          const observer = new MutationObserver(markDirty);
          const observeRoot = () => {
            if (document.documentElement) {
              observer.observe(document.documentElement, {
                subtree: true,
                childList: true,
                attributes: true,
                characterData: true
              });
            }
          };
          observeRoot();
          addEventListener('input', markDirty, true);
          addEventListener('change', markDirty, true);
          addEventListener('scroll', markDirty, true);
          addEventListener('resize', markDirty, true);
          if (typeof ResizeObserver !== 'undefined' && document.documentElement) {
            new ResizeObserver(markDirty).observe(document.documentElement);
          }

          return Object.freeze({version: 1, capture, captureDelta, act, scroll, markDirty});
        })();
        """.formatted(MAX_NODES);

    private DomProgram() {}

    static String install() {
        return "(() => {"
            + "if (!window." + GLOBAL + " || window." + GLOBAL + ".version !== 1) {"
            + CORE
            + "Object.defineProperty(window," + JSONObject.quote(GLOBAL)
            + ",{value:__tempera,writable:false,enumerable:false,configurable:false});}"
            + "return JSON.stringify({ok:true,version:window." + GLOBAL + ".version});})()";
    }

    static String snapshot() {
        return "(() => JSON.stringify(window." + GLOBAL + ".capture()))()";
    }

    static String snapshotDelta(String previousStateHash) {
        return "(() => JSON.stringify(window." + GLOBAL + ".captureDelta("
            + JSONObject.quote(previousStateHash == null ? "" : previousStateHash)
            + ")))()";
    }

    static String action(JSONObject request) {
        return "(() => {const request=JSON.parse(" + JSONObject.quote(request.toString())
            + ");return JSON.stringify(window." + GLOBAL + ".act(request));})()";
    }

    static String scroll(JSONObject request) {
        return "(() => {const request=JSON.parse(" + JSONObject.quote(request.toString())
            + ");return JSON.stringify(window." + GLOBAL + ".scroll(request));})()";
    }
}
