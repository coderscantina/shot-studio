// Renders the mockup artboards in a plain browser, without the claude.ai canvas runtime.
//
// Each `*.dc.html` file loads this script. It implements only what the artboards use:
// `<helmet>` moves into `<head>`, `{{a.b}}` holes in text and attributes, `<sc-for list as>`,
// `<sc-if value>`, `onClick="{{fn}}"`, and a `Component extends DCLogic` class whose
// `setState` re-renders.

class DCLogic {
  constructor(props) {
    this.props = props;
    this.state = {};
  }
  setState(patch) {
    Object.assign(this.state, patch);
    this.__render();
  }
}

const HOLE = /\{\{\s*([^}]+?)\s*\}\}/g;

function lookup(expr, scope) {
  if (expr === "true") return true;
  if (expr === "false") return false;
  return expr.split(".").reduce((value, key) => (value == null ? undefined : value[key]), scope);
}

function fill(text, scope) {
  return text.replace(HOLE, (_, expr) => lookup(expr, scope) ?? "");
}

function onlyHole(text) {
  const match = text.match(/^\s*\{\{\s*([^}]+?)\s*\}\}\s*$/);
  return match && match[1];
}

function expand(node, scope) {
  if (node.nodeType === Node.TEXT_NODE) return [document.createTextNode(fill(node.textContent, scope))];
  if (node.nodeType !== Node.ELEMENT_NODE) return [];

  const tag = node.localName;
  const children = (inner) => [...node.childNodes].flatMap((child) => expand(child, inner));
  if (tag === "sc-for") {
    const list = lookup(onlyHole(node.getAttribute("list")), scope) ?? [];
    const as = node.getAttribute("as");
    return list.flatMap((item) => children({ ...scope, [as]: item }));
  }
  if (tag === "sc-if") return lookup(onlyHole(node.getAttribute("value")), scope) ? children(scope) : [];

  const el = node.cloneNode(false);
  for (const { name, value } of [...node.attributes]) {
    const expr = onlyHole(value);
    if (name.startsWith("on") && expr) {
      el.removeAttribute(name);
      const handler = lookup(expr, scope);
      if (typeof handler === "function") el.addEventListener(name.slice(2), handler);
    } else if (value.includes("{{")) {
      el.setAttribute(name, fill(value, scope));
    }
  }
  el.append(...children(scope));
  return [el];
}

addEventListener("DOMContentLoaded", () => {
  const root = document.querySelector("x-dc");
  const helmet = root.querySelector("helmet");
  if (helmet) {
    document.head.append(...helmet.childNodes);
    helmet.remove();
  }

  const template = document.createElement("template");
  template.innerHTML = root.innerHTML;

  const source = document.querySelector('script[type="text/x-dc"]').textContent;
  const Component = new Function("DCLogic", `${source}\nreturn Component;`)(DCLogic);
  const component = new Component({});
  component.__render = () => root.replaceChildren(...[...template.content.childNodes].flatMap((n) => expand(n, component.renderVals())));
  component.__render();
});
