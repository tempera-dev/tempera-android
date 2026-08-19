package dev.tempera.android.browser;

import org.json.JSONObject;

final class DomProgram {
    private static final int MAX_NODES = 600;
    private static final String GLOBAL = "__temperaAgentRuntimeV1";

    // This program is installed once per document. Hot snapshot/action calls below
    // only invoke the resident functions instead of retransmitting and reparsing
    // the whole semantic runtime on every agent turn.
    private static final String CORE = """
        const __tempera = (() => {
          const MAX_NODES = %d;
          const selector = [
            'a[href]', 'button', 'input', 'textarea', 'select', 'option',
            '[role]', '[tabindex]', '[contenteditable="true"]',
            'summary', 'label', 'video[controls]', 'audio[controls]'
          ].join(',');

          const normalize = value => String(value || '').replace(/\\s+/g, ' ').trim();
          const lower = value => normalize(value).toLowerCase();
          const visible = element => {
            if (!(element instanceof Element)) return false;
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
          const elements = () => Array.from(document.querySelectorAll(selector)).filter(visible).slice(0, MAX_NODES);
          const capture = () => {
            const raw = elements();
            const nodes = raw.map((element, index) => {
              const rect = element.getBoundingClientRect();
              const role = roleOf(element);
              const sensitive = isSensitive(element);
              const value = sensitive ? '' : normalize(element.value || '');
              return {
                ref: '@d' + (index + 1),
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
            const canonical = JSON.stringify({
              url: location.href,
              title: document.title,
              viewport: [innerWidth, innerHeight, devicePixelRatio],
              nodes: nodes.map(node => [node.role,node.label,node.value,node.disabled,node.checked,node.selected,node.sensitive,node.bounds])
            });
            return {
              schemaVersion: 'tempera.android.browser.snapshot/v1',
              url: location.href,
              title: document.title,
              documentStateHash: 'fnv1a64:' + fnv(canonical),
              viewport: {width: innerWidth, height: innerHeight, scale: devicePixelRatio},
              nodes,
              truncated: raw.length >= MAX_NODES,
              trustedForConsequentialActions: false
            };
          };
          const setValue = (element, value) => {
            const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
            const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
            if (descriptor && descriptor.set) descriptor.set.call(element, value);
            else element.value = value;
            element.dispatchEvent(new InputEvent('input', {bubbles: true, inputType: 'insertText', data: value}));
            element.dispatchEvent(new Event('change', {bubbles: true}));
          };
          const act = request => {
            const before = capture();
            if (!request.expectedStateHash || request.expectedStateHash !== before.documentStateHash) {
              return {ok: false, stale: true, error: 'document state changed', before};
            }
            const match = /^@d([1-9][0-9]*)$/.exec(String(request.ref || ''));
            if (!match) return {ok: false, stale: false, error: 'invalid or missing DOM ref', before};
            const list = elements();
            const index = Number(match[1]) - 1;
            const element = list[index];
            if (!element) return {ok: false, stale: true, error: 'DOM ref expired', before};
            const kind = String(request.kind || 'tap');
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
            const after = capture();
            return {
              ok: true,
              stale: false,
              receipt: {
                schemaVersion: 'tempera.android.browser.action-receipt/v1',
                kind,
                ref: request.ref,
                beforeStateHash: before.documentStateHash,
                afterStateHash: after.documentStateHash,
                sensitiveTarget: Boolean(before.nodes[index] && before.nodes[index].sensitive)
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
            scrollBy({left: dx, top: dy, behavior: 'instant'});
            const after = capture();
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
          return Object.freeze({version: 1, capture, act, scroll});
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

    static String action(JSONObject request) {
        return "(() => {const request=JSON.parse(" + JSONObject.quote(request.toString())
            + ");return JSON.stringify(window." + GLOBAL + ".act(request));})()";
    }

    static String scroll(JSONObject request) {
        return "(() => {const request=JSON.parse(" + JSONObject.quote(request.toString())
            + ");return JSON.stringify(window." + GLOBAL + ".scroll(request));})()";
    }
}
